use anyhow::{Context, ensure};
use serde::Deserialize;

use crate::auth::lifecycle::{
    AuthActivationRequest, AuthActivationResult, AuthCatalogInvariantReport, activate_auth_change,
    provider_model_to_select_after_auth, validate_catalog_invariants,
};
use crate::auth::test_sandbox::AuthTestSandbox;
use crate::protocol::{
    AuthChanged, AuthCredentialSource, AuthMethod, CatalogNamespace, RuntimeProviderKey,
};
use crate::provider::ModelRoute;
use crate::provider_catalog::OpenAiCompatibleProfile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AuthLifecycleAuthPath {
    TuiPasteApiKey,
    RemoteTuiPasteApiKey,
    CliLogin,
    EnvFilePreseeded,
    ProcessEnvPreseeded,
}

impl AuthLifecycleAuthPath {
    fn auth_method(self) -> AuthMethod {
        match self {
            Self::TuiPasteApiKey => AuthMethod::TuiPasteApiKey,
            Self::RemoteTuiPasteApiKey => AuthMethod::RemoteTuiPasteApiKey,
            Self::CliLogin => AuthMethod::CliLogin,
            Self::EnvFilePreseeded => AuthMethod::EnvFilePreseeded,
            Self::ProcessEnvPreseeded => AuthMethod::ProcessEnvPreseeded,
        }
    }

    fn credential_source(self) -> AuthCredentialSource {
        match self {
            Self::TuiPasteApiKey
            | Self::RemoteTuiPasteApiKey
            | Self::CliLogin
            | Self::EnvFilePreseeded => AuthCredentialSource::ApiKeyFile,
            Self::ProcessEnvPreseeded => AuthCredentialSource::ProcessEnv,
        }
    }

    fn shows_paste_prompt(self) -> bool {
        matches!(self, Self::TuiPasteApiKey | Self::RemoteTuiPasteApiKey)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AuthLifecycleSpec {
    pub provider_id: &'static str,
    pub provider_label: &'static str,
    pub profile: OpenAiCompatibleProfile,
    pub auth_path: AuthLifecycleAuthPath,
    pub api_key: String,
    pub catalog_models_after_auth: Vec<String>,
    pub selected_model_override: Option<String>,
    pub current_runtime_provider_name: &'static str,
}

impl AuthLifecycleSpec {
    pub(crate) fn cerebras_fixture(auth_path: AuthLifecycleAuthPath) -> Self {
        let mut spec =
            Self::openai_compatible_fixture(crate::provider_catalog::CEREBRAS_PROFILE, auth_path);
        spec.catalog_models_after_auth = vec![
            "qwen-3-235b-a22b-instruct-2507".to_string(),
            "llama3.1-8b".to_string(),
        ];
        spec.selected_model_override = None;
        spec
    }

    pub(crate) fn openai_compatible_fixture(
        profile: OpenAiCompatibleProfile,
        auth_path: AuthLifecycleAuthPath,
    ) -> Self {
        let default_model = profile.default_model.unwrap_or("fixture-model");
        let mut catalog_models_after_auth = vec![default_model.to_string()];
        catalog_models_after_auth.push(format!("{}-alternate-fixture-model", profile.id));
        Self {
            provider_id: profile.id,
            provider_label: profile.display_name,
            profile,
            auth_path,
            api_key: format!("test-{}-key", profile.id),
            catalog_models_after_auth,
            selected_model_override: profile
                .default_model
                .is_none()
                .then(|| default_model.to_string()),
            current_runtime_provider_name: "mock-auth",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PickerSnapshot {
    pub selected_model: Option<String>,
    pub provider_entries: Vec<String>,
    pub switch_target: Option<String>,
    pub switch_request: Option<String>,
    pub switch_route_provider: Option<String>,
    pub switch_route_api_method: Option<String>,
}

impl PickerSnapshot {
    fn build(
        spec: &AuthLifecycleSpec,
        activation: &AuthActivationResult,
        selected_model: Option<&str>,
        routes: &[ModelRoute],
    ) -> Self {
        let provider_routes = routes
            .iter()
            .filter(|route| route.available && route_matches_spec(route, spec))
            .collect::<Vec<_>>();
        let provider_entries = provider_routes
            .iter()
            .map(|route| route.model.clone())
            .collect::<Vec<_>>();
        let selected_model = selected_model
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(ToString::to_string);
        let switch_target = provider_entries
            .iter()
            .find(|model| Some(model.as_str()) != selected_model.as_deref())
            .or_else(|| provider_entries.first())
            .cloned();
        let switch_request = switch_target.as_deref().map(|model| {
            activation.model_switch_request(spec.current_runtime_provider_name, model)
        });
        let switch_route = switch_target.as_ref().and_then(|target| {
            provider_routes
                .iter()
                .find(|route| route.model == *target)
                .copied()
        });

        Self {
            selected_model,
            provider_entries,
            switch_target,
            switch_request,
            switch_route_provider: switch_route.map(|route| route.provider.clone()),
            switch_route_api_method: switch_route.map(|route| route.api_method.clone()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AuthLifecycleResult {
    pub activation: AuthActivationResult,
    pub transcript: Vec<String>,
    pub catalog_report: AuthCatalogInvariantReport,
    pub picker: PickerSnapshot,
    pub catalog_routes: Vec<ModelRoute>,
    pub credential_location: Option<String>,
}

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
            transcript.contains(&format!("{} credentials are active", spec.provider_label)),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            !transcript.contains("OpenAI credentials are active"),
            "{}",
            self.failure_report(spec)
        );
        assert!(
            !transcript.contains("OpenRouter credentials are active"),
            "{}",
            self.failure_report(spec)
        );
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
        let markers = [
            saved_or_detected.as_str(),
            "**Auth Change Received**",
            "**Auth Model Routes Updating**",
            "**Auth Model Catalog Updated**",
        ];
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

pub(crate) struct AuthLifecycleDriver {
    sandbox: AuthTestSandbox,
}

impl AuthLifecycleDriver {
    pub(crate) fn new() -> anyhow::Result<Self> {
        Ok(Self {
            sandbox: AuthTestSandbox::new()?,
        })
    }

    pub(crate) fn run_openai_compatible_fixture(
        &self,
        spec: &AuthLifecycleSpec,
    ) -> anyhow::Result<AuthLifecycleResult> {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(spec.profile);
        ensure!(
            resolved.id == spec.provider_id,
            "spec provider id {} did not match profile {}",
            spec.provider_id,
            resolved.id
        );

        let credential_location = self.apply_credentials(spec, &resolved)?;
        let auth = AuthChanged {
            provider: crate::protocol::AuthProviderId::new(spec.provider_id),
            credential_source: Some(spec.auth_path.credential_source()),
            auth_method: Some(spec.auth_path.auth_method()),
            expected_runtime: Some(RuntimeProviderKey::new("openai-compatible")),
            expected_catalog_namespace: Some(CatalogNamespace::new(spec.provider_id)),
        };
        let activation = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));
        let selected_model = spec
            .selected_model_override
            .clone()
            .or_else(|| activation.activated_model.clone());
        let catalog_routes = self.catalog_routes_for_spec(spec);
        let catalog_report =
            validate_catalog_invariants(&activation, selected_model.as_deref(), &catalog_routes);
        let picker = PickerSnapshot::build(
            spec,
            &activation,
            selected_model.as_deref(),
            &catalog_routes,
        );
        let transcript = self.user_visible_transcript(
            spec,
            &resolved,
            selected_model.as_deref(),
            catalog_report.warning_message().as_deref(),
        );

        Ok(AuthLifecycleResult {
            activation,
            transcript,
            catalog_report,
            picker,
            catalog_routes,
            credential_location,
        })
    }

    fn apply_credentials(
        &self,
        spec: &AuthLifecycleSpec,
        resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
    ) -> anyhow::Result<Option<String>> {
        match spec.auth_path {
            AuthLifecycleAuthPath::TuiPasteApiKey
            | AuthLifecycleAuthPath::RemoteTuiPasteApiKey
            | AuthLifecycleAuthPath::CliLogin
            | AuthLifecycleAuthPath::EnvFilePreseeded => {
                let path = self
                    .sandbox
                    .write_openai_compatible_api_key(spec.profile, &spec.api_key)
                    .with_context(|| format!("write {} API key file", spec.provider_label))?;
                Ok(Some(path.display().to_string()))
            }
            AuthLifecycleAuthPath::ProcessEnvPreseeded => {
                crate::env::set_var(&resolved.api_key_env, &spec.api_key);
                crate::auth::AuthStatus::invalidate_cache();
                Ok(Some(format!("process env {}", resolved.api_key_env)))
            }
        }
    }

    fn catalog_routes_for_spec(&self, spec: &AuthLifecycleSpec) -> Vec<ModelRoute> {
        spec.catalog_models_after_auth
            .iter()
            .map(|model| ModelRoute {
                model: model.clone(),
                provider: spec.provider_label.to_string(),
                api_method: format!("openai-compatible:{}", spec.provider_id),
                available: true,
                detail: "fixture live-catalog route".to_string(),
                cheapness: None,
            })
            .collect()
    }

    fn user_visible_transcript(
        &self,
        spec: &AuthLifecycleSpec,
        resolved: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
        selected_model: Option<&str>,
        warning: Option<&str>,
    ) -> Vec<String> {
        let mut transcript = Vec::new();
        if spec.auth_path.shows_paste_prompt() {
            transcript.push(format!(
                "**{} API Key**\n\nSetup docs: {}\nStored variable: `{}`\nEndpoint: `{}`\nSuggested default model: `{}`\n\n**Paste your API key below** (it will be saved securely), or type `/cancel` to abort.",
                spec.provider_label,
                resolved.setup_url,
                resolved.api_key_env,
                resolved.api_base,
                resolved.default_model.as_deref().unwrap_or("none")
            ));
            transcript.push(format!(
                "**{} API key saved.**\n\nStored at `{}`.\nFetching models now. Jcode will switch to an accessible model returned by the live catalog and show the catalog diff when discovery finishes.",
                spec.provider_label,
                self.sandbox.env_file_path(&resolved.env_file).display()
            ));
        } else {
            transcript.push(format!(
                "**{} credentials detected.**\n\nCredential source: {:?}. Fetching models now.",
                spec.provider_label,
                spec.auth_path.credential_source()
            ));
        }
        transcript.push(
            "**Auth Change Received**\n\nThe server is reloading provider credentials and refreshing model route availability for this session."
                .to_string(),
        );
        transcript.push(
            "**Auth Model Routes Updating**\n\nCredentials are reloaded. Jcode is pushing an updated model catalog snapshot to connected clients."
                .to_string(),
        );
        let mut updated = format!(
            "**Auth Model Catalog Updated**\n\n{} credentials are active. Catalog diff:\n\nModels: fixture-before → fixture-after\nRoutes: fixture-before → fixture-after\n\nSelected model: `{}`.",
            spec.provider_label,
            selected_model.unwrap_or("none")
        );
        if let Some(warning) = warning {
            updated.push_str(warning);
        }
        transcript.push(updated);
        transcript
    }
}

fn route_matches_spec(route: &ModelRoute, spec: &AuthLifecycleSpec) -> bool {
    route
        .api_method
        .eq_ignore_ascii_case(&format!("openai-compatible:{}", spec.provider_id))
        || route.api_method.eq_ignore_ascii_case(spec.provider_id)
}

// The live provider probes were moved to `live_provider_probes` so they compile
// into the shipping binary (used by `provider_e2e`). Re-export for the tests
// below that reference them via `super::*`.
#[cfg(test)]
pub(crate) use crate::auth::live_provider_probes::{
    fetch_live_openai_compatible_models, run_live_openai_compatible_smoke,
    run_live_openai_compatible_stream_smoke, run_live_openai_compatible_tool_smoke,
};

#[cfg(test)]
mod tests {
    use super::*;

    /// True when an OpenAI-compatible profile id is intentionally remapped onto a
    /// native runtime (e.g. `anthropic-api` -> `claude-api`, `openai-api`) instead
    /// of the generic `openai-compatible:<id>` route. Such profiles exist so
    /// `provider-doctor` can drive the native Anthropic/OpenAI API-key surfaces,
    /// but they fail the generic openai-compatible lifecycle contracts by design,
    /// so the generic matrices skip them.
    fn is_native_routed_compat_profile(
        profile: &crate::provider_catalog::OpenAiCompatibleProfile,
    ) -> bool {
        crate::auth::lifecycle::normalized_auth_provider_id(Some(profile.id))
            .is_some_and(|canonical| canonical != profile.id)
    }

    fn env_truthy(key: &str) -> bool {
        std::env::var(key)
            .ok()
            .map(|value| {
                let value = value.trim();
                !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
    }

    struct LiveTestApiKey {
        secret: String,
        auth: crate::live_tests::LiveVerificationAuth,
    }

    fn live_cerebras_api_key() -> Option<LiveTestApiKey> {
        std::env::var("JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .map(|secret| LiveTestApiKey {
                auth: crate::live_tests::LiveVerificationAuth::from_secret(
                    "env:JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY",
                    Some("JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY"),
                    &secret,
                ),
                secret,
            })
    }

    fn live_opencode_zen_api_key() -> Option<LiveTestApiKey> {
        if let Some(secret) = std::env::var("JCODE_AUTH_LIFECYCLE_OPENCODE_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        {
            return Some(LiveTestApiKey {
                auth: crate::live_tests::LiveVerificationAuth::from_secret(
                    "env:JCODE_AUTH_LIFECYCLE_OPENCODE_API_KEY",
                    Some("JCODE_AUTH_LIFECYCLE_OPENCODE_API_KEY"),
                    &secret,
                ),
                secret,
            });
        }

        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(
            crate::provider_catalog::OPENCODE_PROFILE,
        );
        crate::provider_catalog::load_api_key_from_env_or_config(
            &resolved.api_key_env,
            &resolved.env_file,
        )
        .map(|secret| LiveTestApiKey {
            auth: crate::live_tests::LiveVerificationAuth::from_secret(
                format!("{} via {}", resolved.api_key_env, resolved.env_file),
                Some(resolved.api_key_env.clone()),
                &secret,
            ),
            secret,
        })
    }

    fn live_openai_compatible_api_key(profile: OpenAiCompatibleProfile) -> Option<LiveTestApiKey> {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        crate::provider_catalog::load_api_key_from_env_or_config(
            &resolved.api_key_env,
            &resolved.env_file,
        )
        .map(|secret| LiveTestApiKey {
            auth: crate::live_tests::LiveVerificationAuth::from_secret(
                format!("{} via {}", resolved.api_key_env, resolved.env_file),
                Some(resolved.api_key_env.clone()),
                &secret,
            ),
            secret,
        })
    }

    fn live_event<I, S>(
        test_name: &str,
        profile: OpenAiCompatibleProfile,
        auth: crate::live_tests::LiveVerificationAuth,
        model: Option<&str>,
        capabilities: I,
        result: crate::live_tests::LiveVerificationResult,
    ) -> crate::live_tests::LiveVerificationEvent
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        let mut event = crate::live_tests::LiveVerificationEvent::new(
            test_name,
            resolved.id,
            resolved.display_name,
            auth,
            result,
        )
        .with_endpoint(resolved.api_base)
        .with_capabilities(capabilities)
        .with_standard_end_to_end_checkpoints()
        .with_metadata(
            "cost_policy",
            serde_json::json!("env_gated_may_spend_balance"),
        );
        if let Some(model) = model {
            event = event.with_model(model);
        }
        event
    }

    fn append_live_event(event: &crate::live_tests::LiveVerificationEvent) {
        let event = event.clone().with_not_run_for_missing_expected_checkpoints(
            "checkpoint not exercised by this live test invocation",
        );
        let paths = crate::live_tests::append_event(&event)
            .expect("append live verification evidence ledger");
        eprintln!(
            "live verification recorded: events={} coverage={} user_ready={} gaps={:?}",
            paths.events.display(),
            paths.coverage.display(),
            event.user_ready(),
            event.readiness_gaps()
        );
    }

    fn cost_quota_safety_stage(spend_enabled: bool) -> crate::live_tests::LiveVerificationStage {
        crate::live_tests::LiveVerificationStage::passed(
            crate::live_tests::checkpoints::COST_QUOTA_SAFETY,
        )
        .with_evidence("live_tests_env_gated", serde_json::json!(true))
        .with_evidence("spend_enabled", serde_json::json!(spend_enabled))
        .with_evidence("auth_secret_logged", serde_json::json!(false))
        .with_evidence(
            "usage_cost_recorded_when_available",
            serde_json::json!(true),
        )
    }

    fn covered_stage_names(stages: &[crate::live_tests::LiveVerificationStage]) -> Vec<String> {
        stages
            .iter()
            .filter(|stage| stage.status != crate::live_tests::LiveVerificationStageStatus::NotRun)
            .map(|stage| stage.name.clone())
            .collect()
    }

    fn stale_openai_route(model: &str) -> ModelRoute {
        ModelRoute {
            model: model.to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openai".to_string(),
            available: true,
            detail: "stale route".to_string(),
            cheapness: None,
        }
    }

    fn assert_rejected_success(
        spec: &AuthLifecycleSpec,
        result: AuthLifecycleResult,
        scenario: &str,
        expected_message: &str,
    ) {
        let panic = std::panic::catch_unwind(|| result.assert_success(spec))
            .expect_err("degraded state must not satisfy happy auth lifecycle");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        assert!(
            message.contains(expected_message),
            "unexpected assertion for {scenario}: expected `{expected_message}` in:\n{message}"
        );
    }

    #[test]
    fn cerebras_remote_tui_paste_key_fixture_covers_catalog_picker_and_switch() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);

        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");

        result.assert_success(&spec);
        assert!(result.transcript_text().contains("**Cerebras API Key**"));
        assert!(
            result
                .transcript_text()
                .contains("**Cerebras API key saved.**")
        );
        assert_eq!(
            result.picker.selected_model.as_deref(),
            Some("qwen-3-235b-a22b-instruct-2507")
        );
        assert_eq!(result.picker.switch_target.as_deref(), Some("llama3.1-8b"));
        assert_eq!(
            result.picker.switch_request.as_deref(),
            Some("cerebras:llama3.1-8b")
        );
    }

    #[test]
    fn cerebras_state_space_catches_stale_openai_catalog_after_auth() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let mut spec =
            AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        spec.catalog_models_after_auth.clear();
        spec.selected_model_override = Some("gpt-5.5".to_string());

        let mut result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");
        result.catalog_routes = vec![stale_openai_route("gpt-5.5")];
        result.catalog_report = validate_catalog_invariants(
            &result.activation,
            result.picker.selected_model.as_deref(),
            &result.catalog_routes,
        );
        result.picker = PickerSnapshot::build(
            &spec,
            &result.activation,
            result.picker.selected_model.as_deref(),
            &result.catalog_routes,
        );

        assert!(!result.catalog_report.ok());
        let failure = result.failure_report(&spec);
        assert!(failure.contains("Expected selectable Cerebras model routes"));
        assert!(failure.contains("Selected model: `gpt-5.5`"));
        assert!(failure.contains("OpenAI"));
    }

    #[test]
    fn auth_lifecycle_failure_contracts_reject_degraded_success_states() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        let success = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");

        let mut invalid_key = success.clone();
        invalid_key.transcript.push(
            "**Login: Cerebras failed**\n\nInvalid API key. No model catalog was activated."
                .to_string(),
        );
        assert_rejected_success(&spec, invalid_key, "invalid api key", "failed");

        let mut network_failure = success.clone();
        network_failure.transcript = vec![
            format!("**{} API key saved.**", spec.provider_label),
            "**Auth Change Received**\n\nThe server is reloading provider credentials.".to_string(),
            "**Model Discovery Still Updating**\n\nCould not fetch the live catalog yet; waiting for server refresh."
                .to_string(),
        ];
        assert_rejected_success(
            &spec,
            network_failure,
            "network catalog failure pending state",
            "Model Discovery Still Updating",
        );

        let mut empty_catalog = success.clone();
        empty_catalog.catalog_routes.clear();
        empty_catalog.catalog_report = validate_catalog_invariants(
            &empty_catalog.activation,
            empty_catalog.picker.selected_model.as_deref(),
            &empty_catalog.catalog_routes,
        );
        empty_catalog.picker = PickerSnapshot::build(
            &spec,
            &empty_catalog.activation,
            empty_catalog.picker.selected_model.as_deref(),
            &empty_catalog.catalog_routes,
        );
        assert_rejected_success(&spec, empty_catalog, "empty catalog", "catalog invariant");

        let mut wrong_profile = success.clone();
        for route in &mut wrong_profile.catalog_routes {
            route.api_method = "openai-compatible:other-provider".to_string();
            route.detail = "wrong profile live-catalog route".to_string();
        }
        wrong_profile.catalog_report = validate_catalog_invariants(
            &wrong_profile.activation,
            wrong_profile.picker.selected_model.as_deref(),
            &wrong_profile.catalog_routes,
        );
        wrong_profile.picker = PickerSnapshot::build(
            &spec,
            &wrong_profile.activation,
            wrong_profile.picker.selected_model.as_deref(),
            &wrong_profile.catalog_routes,
        );
        assert_rejected_success(
            &spec,
            wrong_profile,
            "wrong provider profile catalog",
            "catalog invariant",
        );

        let mut stale_cached_catalog = success.clone();
        stale_cached_catalog.catalog_routes = vec![stale_openai_route("gpt-5.5")];
        stale_cached_catalog.catalog_report = validate_catalog_invariants(
            &stale_cached_catalog.activation,
            Some("gpt-5.5"),
            &stale_cached_catalog.catalog_routes,
        );
        stale_cached_catalog.picker = PickerSnapshot::build(
            &spec,
            &stale_cached_catalog.activation,
            Some("gpt-5.5"),
            &stale_cached_catalog.catalog_routes,
        );
        assert_rejected_success(
            &spec,
            stale_cached_catalog,
            "stale cached OpenAI catalog",
            "catalog invariant",
        );
    }

    #[test]
    fn cerebras_env_file_and_process_env_paths_share_same_lifecycle_invariants() {
        for auth_path in [
            AuthLifecycleAuthPath::TuiPasteApiKey,
            AuthLifecycleAuthPath::CliLogin,
            AuthLifecycleAuthPath::EnvFilePreseeded,
            AuthLifecycleAuthPath::ProcessEnvPreseeded,
        ] {
            let driver = AuthLifecycleDriver::new().expect("driver");
            let spec = AuthLifecycleSpec::cerebras_fixture(auth_path);

            let result = driver
                .run_openai_compatible_fixture(&spec)
                .expect("lifecycle result");

            result.assert_success(&spec);
            if auth_path.shows_paste_prompt() {
                assert!(result.transcript_text().contains("**Cerebras API Key**"));
            } else {
                assert!(
                    result
                        .transcript_text()
                        .contains("**Cerebras credentials detected.**")
                );
            }
        }
    }

    #[test]
    fn openai_compatible_provider_matrix_preserves_identity_catalog_and_picker() {
        let auth_paths = [
            AuthLifecycleAuthPath::RemoteTuiPasteApiKey,
            AuthLifecycleAuthPath::CliLogin,
            AuthLifecycleAuthPath::EnvFilePreseeded,
            AuthLifecycleAuthPath::ProcessEnvPreseeded,
        ];

        for profile in crate::provider_catalog::openai_compatible_profiles()
            .iter()
            .copied()
            .filter(|profile| !is_native_routed_compat_profile(profile))
        {
            for auth_path in auth_paths {
                let driver = AuthLifecycleDriver::new().unwrap_or_else(|error| {
                    panic!(
                        "driver for provider {} via {:?}: {error:?}",
                        profile.id, auth_path
                    )
                });
                let spec = AuthLifecycleSpec::openai_compatible_fixture(profile, auth_path);

                let result = driver
                    .run_openai_compatible_fixture(&spec)
                    .unwrap_or_else(|error| {
                        panic!(
                            "lifecycle setup failed for provider {} via {:?}: {error:?}",
                            profile.id, auth_path
                        )
                    });

                result.assert_success(&spec);
                assert!(
                    result
                        .picker
                        .switch_request
                        .as_deref()
                        .is_some_and(|request| request.starts_with(&format!("{}:", profile.id))),
                    "{}",
                    result.failure_report(&spec)
                );
            }
        }
    }

    #[test]
    fn provider_switch_reauth_matrix_recovers_from_stale_previous_provider_state() {
        // Native-routed profiles (Anthropic/OpenAI API-key) deliberately use a
        // native runtime route, not the generic `openai-compatible:<id>` one, so
        // exclude them from this generic switch/reauth contract.
        let profiles: Vec<_> = crate::provider_catalog::openai_compatible_profiles()
            .iter()
            .copied()
            .filter(|profile| !is_native_routed_compat_profile(profile))
            .collect();
        assert!(
            profiles.len() >= 2,
            "switch/reauth matrix needs at least two OpenAI-compatible providers"
        );

        for window in profiles.windows(2) {
            let previous_profile = window[0];
            let reauth_profile = window[1];
            let driver = AuthLifecycleDriver::new().expect("driver");
            let previous_spec = AuthLifecycleSpec::openai_compatible_fixture(
                previous_profile,
                AuthLifecycleAuthPath::RemoteTuiPasteApiKey,
            );
            let reauth_spec = AuthLifecycleSpec::openai_compatible_fixture(
                reauth_profile,
                AuthLifecycleAuthPath::RemoteTuiPasteApiKey,
            );

            let previous = driver
                .run_openai_compatible_fixture(&previous_spec)
                .unwrap_or_else(|error| {
                    panic!(
                        "previous provider {} setup failed: {error:?}",
                        previous_spec.provider_id
                    )
                });
            let reauth = driver
                .run_openai_compatible_fixture(&reauth_spec)
                .unwrap_or_else(|error| {
                    panic!(
                        "reauth provider {} setup failed: {error:?}",
                        reauth_spec.provider_id
                    )
                });

            let stale_selected_model = previous.picker.selected_model.as_deref();
            let mut mixed_routes = previous.catalog_routes.clone();
            mixed_routes.extend(reauth.catalog_routes.clone());
            let session_model_after_reauth = provider_model_to_select_after_auth(
                &reauth.activation,
                stale_selected_model,
                &mixed_routes,
            )
            .or_else(|| stale_selected_model.map(ToString::to_string));
            let catalog_report = validate_catalog_invariants(
                &reauth.activation,
                session_model_after_reauth.as_deref(),
                &mixed_routes,
            );
            let picker = PickerSnapshot::build(
                &reauth_spec,
                &reauth.activation,
                session_model_after_reauth.as_deref(),
                &mixed_routes,
            );

            assert!(
                catalog_report.ok(),
                "reauth of {} after {} left stale selected/catalog state: {:?}",
                reauth_spec.provider_id,
                previous_spec.provider_id,
                catalog_report.warning_message()
            );
            assert!(
                session_model_after_reauth
                    .as_ref()
                    .is_some_and(|selected| picker
                        .provider_entries
                        .iter()
                        .any(|entry| entry == selected)),
                "reauth of {} after {} selected {:?}, picker entries {:?}",
                reauth_spec.provider_id,
                previous_spec.provider_id,
                session_model_after_reauth,
                picker.provider_entries
            );
            assert!(
                picker.provider_entries.iter().all(|entry| reauth
                    .catalog_routes
                    .iter()
                    .any(|route| route.model == *entry)),
                "reauth picker for {} leaked previous provider {} entries: {:?}",
                reauth_spec.provider_id,
                previous_spec.provider_id,
                picker.provider_entries
            );
            assert!(
                picker.switch_request.as_deref().is_some_and(
                    |request| request.starts_with(&format!("{}:", reauth_spec.provider_id))
                ),
                "reauth picker switch must target {}, got {:?}",
                reauth_spec.provider_id,
                picker.switch_request
            );
        }
    }

    #[test]
    fn picker_switch_target_uses_profile_route_not_matching_label_only_route() {
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        let auth = AuthChanged {
            provider: crate::protocol::AuthProviderId::new(spec.provider_id),
            credential_source: Some(spec.auth_path.credential_source()),
            auth_method: Some(spec.auth_path.auth_method()),
            expected_runtime: Some(RuntimeProviderKey::new("openai-compatible")),
            expected_catalog_namespace: Some(CatalogNamespace::new(spec.provider_id)),
        };
        let activation = activate_auth_change(&AuthActivationRequest::new(None, Some(auth)));
        let routes = vec![
            ModelRoute {
                model: "wrong-profile-first".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible:other-provider".to_string(),
                available: true,
                detail: "wrong namespace".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "qwen-3-235b-a22b-instruct-2507".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible:cerebras".to_string(),
                available: true,
                detail: "correct namespace".to_string(),
                cheapness: None,
            },
            ModelRoute {
                model: "llama3.1-8b".to_string(),
                provider: "Cerebras".to_string(),
                api_method: "openai-compatible:cerebras".to_string(),
                available: true,
                detail: "correct namespace".to_string(),
                cheapness: None,
            },
        ];

        let picker = PickerSnapshot::build(
            &spec,
            &activation,
            Some("qwen-3-235b-a22b-instruct-2507"),
            &routes,
        );

        assert_eq!(
            picker.provider_entries,
            vec![
                "qwen-3-235b-a22b-instruct-2507".to_string(),
                "llama3.1-8b".to_string()
            ]
        );
        assert_eq!(picker.switch_target.as_deref(), Some("llama3.1-8b"));
        assert_eq!(
            picker.switch_route_api_method.as_deref(),
            Some("openai-compatible:cerebras")
        );
        assert!(
            !picker
                .provider_entries
                .iter()
                .any(|model| model == "wrong-profile-first")
        );
    }

    #[test]
    fn auth_lifecycle_success_rejects_static_fallback_route_sources() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        let mut result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");
        for route in &mut result.catalog_routes {
            route.detail = "fixture static fallback route".to_string();
        }

        assert!(
            result.catalog_report.ok(),
            "the catalog shape is valid, so only source attribution should fail"
        );
        let panic = std::panic::catch_unwind(|| result.assert_success(&spec))
            .expect_err("static fallback routes must not satisfy happy auth lifecycle");
        let message = panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        assert!(
            message.contains("static fallback"),
            "unexpected assertion failure: {message}"
        );
    }

    #[test]
    fn auth_lifecycle_success_rejects_provider_routes_not_returned_by_live_catalog() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        let mut result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");
        result.catalog_routes.push(ModelRoute {
            model: "zai-glm-4.7".to_string(),
            provider: "Cerebras".to_string(),
            api_method: "openai-compatible:cerebras".to_string(),
            available: true,
            detail: "https://api.cerebras.ai/v1".to_string(),
            cheapness: None,
        });
        result.catalog_report = validate_catalog_invariants(
            &result.activation,
            result.picker.selected_model.as_deref(),
            &result.catalog_routes,
        );
        result.picker = PickerSnapshot::build(
            &spec,
            &result.activation,
            result.picker.selected_model.as_deref(),
            &result.catalog_routes,
        );

        assert!(
            result.catalog_report.ok(),
            "the route shape is valid, so only live-catalog membership should fail"
        );
        assert!(
            result
                .picker
                .provider_entries
                .iter()
                .any(|model| model == "zai-glm-4.7"),
            "test setup should mimic a stale static/provider route leaking into /model"
        );
        assert_rejected_success(
            &spec,
            result,
            "provider route absent from live catalog",
            "not returned by the live catalog",
        );
    }

    #[test]
    fn auth_lifecycle_success_rejects_duplicate_or_out_of_order_transcript_markers() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("lifecycle result");

        let mut duplicated = result.clone();
        duplicated
            .transcript
            .push("**Auth Model Catalog Updated**\n\nDuplicate final success.".to_string());
        let duplicate_panic = std::panic::catch_unwind(|| duplicated.assert_success(&spec))
            .expect_err("duplicate final catalog update must not satisfy happy auth lifecycle");
        let duplicate_message = duplicate_panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| duplicate_panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        assert!(
            duplicate_message.contains("duplicate"),
            "unexpected assertion failure: {duplicate_message}"
        );

        let mut out_of_order = result.clone();
        out_of_order.transcript.swap(1, 3);
        let order_panic = std::panic::catch_unwind(|| out_of_order.assert_success(&spec))
            .expect_err("out-of-order auth transcript must not satisfy happy lifecycle");
        let order_message = order_panic
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| order_panic.downcast_ref::<&str>().copied())
            .unwrap_or("unknown panic");
        assert!(
            order_message.contains("out of order"),
            "unexpected assertion failure: {order_message}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn cerebras_live_opt_in_catalog_lifecycle_uses_isolated_sandbox() {
        if !env_truthy("JCODE_AUTH_LIFECYCLE_LIVE") {
            eprintln!(
                "skipping live Cerebras auth lifecycle test; set JCODE_AUTH_LIFECYCLE_LIVE=1 and JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY"
            );
            return;
        }
        let api_key = live_cerebras_api_key()
            .expect("JCODE_AUTH_LIFECYCLE_LIVE=1 requires JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY");

        let spend_smoke = env_truthy("JCODE_AUTH_LIFECYCLE_SMOKE");
        let stream_smoke = env_truthy("JCODE_AUTH_LIFECYCLE_STREAM_SMOKE");
        let mut stages = vec![
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_CREDENTIAL_LOADED,
            )
            .with_evidence(
                "auth_source",
                serde_json::json!(api_key.auth.source.clone()),
            )
            .with_evidence("env_key", serde_json::json!(api_key.auth.env_key.clone())),
            cost_quota_safety_stage(spend_smoke || stream_smoke),
        ];
        let models_result = fetch_live_openai_compatible_models(
            crate::provider_catalog::CEREBRAS_PROFILE,
            &api_key.secret,
        )
        .await;
        let models = match models_result {
            Ok(models) => models,
            Err(error) => {
                stages.push(crate::live_tests::LiveVerificationStage::failed(
                    crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    error.to_string(),
                ));
                let capabilities = covered_stage_names(&stages);
                let event = live_event(
                    "cerebras_live_opt_in_catalog_lifecycle_uses_isolated_sandbox",
                    crate::provider_catalog::CEREBRAS_PROFILE,
                    api_key.auth.clone(),
                    None,
                    capabilities,
                    crate::live_tests::LiveVerificationResult::Failed,
                )
                .with_stages(stages);
                append_live_event(&event);
                panic!("live Cerebras model catalog: {error:?}");
            }
        };
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            )
            .with_evidence(
                "models",
                crate::live_tests::concise_model_sample(&models, 12),
            ),
        );
        let default_model = crate::provider_catalog::CEREBRAS_PROFILE.default_model;
        let selected = default_model
            .filter(|default| models.iter().any(|model| model == default))
            .map(ToString::to_string)
            .or_else(|| models.first().cloned())
            .expect("live catalog has model");

        let driver = AuthLifecycleDriver::new().expect("driver");
        let mut spec =
            AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::RemoteTuiPasteApiKey);
        spec.api_key = api_key.secret.clone();
        spec.catalog_models_after_auth = models;
        spec.selected_model_override = Some(selected.clone());

        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("live lifecycle result");

        result.assert_success(&spec);
        assert!(
            result
                .catalog_routes
                .iter()
                .any(|route| route.model == selected && route.provider == "Cerebras"),
            "{}",
            result.failure_report(&spec)
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_UX_KEY_ENTRY,
            )
            .with_evidence("auth_path", serde_json::json!("remote_tui_paste_api_key"))
            .with_evidence("simulated_in_sandbox", serde_json::json!(true))
            .with_evidence("transcript_order_verified", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CREDENTIAL_PERSISTENCE,
            )
            .with_evidence(
                "credential_location",
                serde_json::json!(result.credential_location.clone()),
            )
            .with_evidence("sandboxed", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            )
            .with_evidence("transcript_markers_verified", serde_json::json!(true))
            .with_evidence(
                "catalog_route_count",
                serde_json::json!(result.catalog_routes.len()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_LIVE_MODELS,
            )
            .with_evidence("selected_model", serde_json::json!(selected.clone()))
            .with_evidence(
                "picker_entries",
                serde_json::json!(result.picker.provider_entries.clone()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_FALLBACK_LABELING,
            )
            .with_evidence("fallback_routes_present", serde_json::json!(false))
            .with_evidence(
                "all_picker_entries_from_live_catalog",
                serde_json::json!(true),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_SWITCH_ROUTE,
            )
            .with_evidence(
                "switch_request",
                serde_json::json!(result.picker.switch_request.clone()),
            )
            .with_evidence(
                "switch_route_api_method",
                serde_json::json!(result.picker.switch_route_api_method.clone()),
            ),
        );

        if spend_smoke {
            match run_live_openai_compatible_smoke(
                crate::provider_catalog::CEREBRAS_PROFILE,
                &api_key.secret,
                &selected,
            )
            .await
            {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    stages.push(crate::live_tests::LiveVerificationStage::failed(
                        crate::live_tests::checkpoints::NON_STREAMING_CHAT_COMPLETION,
                        error.to_string(),
                    ));
                    let capabilities = covered_stage_names(&stages);
                    let event = live_event(
                        "cerebras_live_opt_in_catalog_lifecycle_uses_isolated_sandbox",
                        crate::provider_catalog::CEREBRAS_PROFILE,
                        api_key.auth.clone(),
                        Some(&selected),
                        capabilities,
                        crate::live_tests::LiveVerificationResult::Failed,
                    )
                    .with_stages(stages);
                    append_live_event(&event);
                    panic!("live Cerebras smoke completion: {error:?}");
                }
            }
        }

        if stream_smoke {
            match run_live_openai_compatible_stream_smoke(
                crate::provider_catalog::CEREBRAS_PROFILE,
                &api_key.secret,
                &selected,
            )
            .await
            {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    stages.push(crate::live_tests::LiveVerificationStage::failed(
                        crate::live_tests::checkpoints::STREAMING_CHAT_COMPLETION,
                        error.to_string(),
                    ));
                    let capabilities = covered_stage_names(&stages);
                    let event = live_event(
                        "cerebras_live_opt_in_catalog_lifecycle_uses_isolated_sandbox",
                        crate::provider_catalog::CEREBRAS_PROFILE,
                        api_key.auth.clone(),
                        Some(&selected),
                        capabilities,
                        crate::live_tests::LiveVerificationResult::Failed,
                    )
                    .with_stages(stages);
                    append_live_event(&event);
                    panic!("live Cerebras stream smoke completion: {error:?}");
                }
            }
        }

        let capabilities = covered_stage_names(&stages);
        let event = live_event(
            "cerebras_live_opt_in_catalog_lifecycle_uses_isolated_sandbox",
            crate::provider_catalog::CEREBRAS_PROFILE,
            api_key.auth.clone(),
            Some(&selected),
            capabilities,
            crate::live_tests::LiveVerificationResult::Passed,
        )
        .with_stages(stages);
        append_live_event(&event);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn opencode_zen_live_opt_in_tool_call_smoke() {
        if !env_truthy("JCODE_OPENCODE_ZEN_LIVE_TOOL_TEST") {
            eprintln!(
                "skipping live OpenCode Zen tool-call smoke; set JCODE_OPENCODE_ZEN_LIVE_TOOL_TEST=1 and provide OPENCODE_API_KEY"
            );
            return;
        }
        let api_key = live_opencode_zen_api_key().expect(
            "JCODE_OPENCODE_ZEN_LIVE_TOOL_TEST=1 requires OPENCODE_API_KEY or JCODE_AUTH_LIFECYCLE_OPENCODE_API_KEY",
        );
        let model = std::env::var("JCODE_OPENCODE_ZEN_LIVE_TOOL_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "kimi-k2.6".to_string());

        let stream_smoke = env_truthy("JCODE_OPENCODE_ZEN_LIVE_STREAM_TEST")
            || env_truthy("JCODE_AUTH_LIFECYCLE_STREAM_SMOKE");
        let mut stages = vec![
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_CREDENTIAL_LOADED,
            )
            .with_evidence(
                "auth_source",
                serde_json::json!(api_key.auth.source.clone()),
            )
            .with_evidence("env_key", serde_json::json!(api_key.auth.env_key.clone())),
            cost_quota_safety_stage(true),
        ];
        let models_result = fetch_live_openai_compatible_models(
            crate::provider_catalog::OPENCODE_PROFILE,
            &api_key.secret,
        )
        .await;
        let models = match models_result {
            Ok(models) => models,
            Err(error) => {
                stages.push(crate::live_tests::LiveVerificationStage::failed(
                    crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    error.to_string(),
                ));
                let capabilities = covered_stage_names(&stages);
                let event = live_event(
                    "opencode_zen_live_opt_in_tool_call_smoke",
                    crate::provider_catalog::OPENCODE_PROFILE,
                    api_key.auth.clone(),
                    Some(&model),
                    capabilities,
                    crate::live_tests::LiveVerificationResult::Failed,
                )
                .with_stages(stages);
                append_live_event(&event);
                panic!("live OpenCode Zen model catalog: {error:?}");
            }
        };
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            )
            .with_evidence(
                "models",
                crate::live_tests::concise_model_sample(&models, 16),
            )
            .with_evidence(
                "requested_model_present",
                serde_json::json!(models.contains(&model)),
            ),
        );
        assert!(
            models.iter().any(|candidate| candidate == &model),
            "live OpenCode Zen catalog did not include requested test model `{model}`"
        );

        let driver = AuthLifecycleDriver::new().expect("driver");
        let mut spec = AuthLifecycleSpec::openai_compatible_fixture(
            crate::provider_catalog::OPENCODE_PROFILE,
            AuthLifecycleAuthPath::RemoteTuiPasteApiKey,
        );
        spec.api_key = api_key.secret.clone();
        spec.catalog_models_after_auth = models.clone();
        spec.selected_model_override = Some(model.clone());
        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("live OpenCode Zen auth lifecycle fixture");
        result.assert_success(&spec);
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_UX_KEY_ENTRY,
            )
            .with_evidence("auth_path", serde_json::json!("remote_tui_paste_api_key"))
            .with_evidence("simulated_in_sandbox", serde_json::json!(true))
            .with_evidence("transcript_order_verified", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CREDENTIAL_PERSISTENCE,
            )
            .with_evidence(
                "credential_location",
                serde_json::json!(result.credential_location.clone()),
            )
            .with_evidence("sandboxed", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            )
            .with_evidence("transcript_markers_verified", serde_json::json!(true))
            .with_evidence(
                "catalog_route_count",
                serde_json::json!(result.catalog_routes.len()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_LIVE_MODELS,
            )
            .with_evidence("selected_model", serde_json::json!(model.clone()))
            .with_evidence(
                "picker_entries",
                serde_json::json!(result.picker.provider_entries.clone()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_FALLBACK_LABELING,
            )
            .with_evidence("fallback_routes_present", serde_json::json!(false))
            .with_evidence(
                "all_picker_entries_from_live_catalog",
                serde_json::json!(true),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_SWITCH_ROUTE,
            )
            .with_evidence(
                "switch_request",
                serde_json::json!(result.picker.switch_request.clone()),
            )
            .with_evidence(
                "switch_route_api_method",
                serde_json::json!(result.picker.switch_route_api_method.clone()),
            ),
        );

        if stream_smoke {
            match run_live_openai_compatible_stream_smoke(
                crate::provider_catalog::OPENCODE_PROFILE,
                &api_key.secret,
                &model,
            )
            .await
            {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    stages.push(crate::live_tests::LiveVerificationStage::failed(
                        crate::live_tests::checkpoints::STREAMING_CHAT_COMPLETION,
                        error.to_string(),
                    ));
                    let capabilities = covered_stage_names(&stages);
                    let event = live_event(
                        "opencode_zen_live_opt_in_tool_call_smoke",
                        crate::provider_catalog::OPENCODE_PROFILE,
                        api_key.auth.clone(),
                        Some(&model),
                        capabilities,
                        crate::live_tests::LiveVerificationResult::Failed,
                    )
                    .with_stages(stages);
                    append_live_event(&event);
                    panic!("live OpenCode Zen stream smoke: {error:?}");
                }
            }
        }

        match run_live_openai_compatible_tool_smoke(
            crate::provider_catalog::OPENCODE_PROFILE,
            &api_key.secret,
            &model,
        )
        .await
        {
            Ok(stage) => stages.push(stage),
            Err(error) => {
                stages.push(crate::live_tests::LiveVerificationStage::failed(
                    crate::live_tests::checkpoints::TOOL_CALL_PARSE,
                    error.to_string(),
                ));
                let capabilities = covered_stage_names(&stages);
                let event = live_event(
                    "opencode_zen_live_opt_in_tool_call_smoke",
                    crate::provider_catalog::OPENCODE_PROFILE,
                    api_key.auth.clone(),
                    Some(&model),
                    capabilities,
                    crate::live_tests::LiveVerificationResult::Failed,
                )
                .with_stages(stages);
                append_live_event(&event);
                panic!("live OpenCode Zen tool-call smoke: {error:?}");
            }
        }

        let capabilities = covered_stage_names(&stages);
        let event = live_event(
            "opencode_zen_live_opt_in_tool_call_smoke",
            crate::provider_catalog::OPENCODE_PROFILE,
            api_key.auth.clone(),
            Some(&model),
            capabilities,
            crate::live_tests::LiveVerificationResult::Passed,
        )
        .with_stages(stages)
        .with_metadata("default_model", serde_json::json!("kimi-k2.6"));
        append_live_event(&event);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn issue_driven_openai_compatible_live_target_smoke() {
        let Some(provider_id) = std::env::var("JCODE_ISSUE_DRIVEN_LIVE_PROVIDER")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            eprintln!(
                "skipping issue-driven live provider test; set JCODE_ISSUE_DRIVEN_LIVE_PROVIDER to an OpenAI-compatible profile id"
            );
            return;
        };
        let profile = crate::provider_catalog::openai_compatible_profile_by_id(&provider_id)
            .unwrap_or_else(|| panic!("unknown OpenAI-compatible profile id: {provider_id}"));
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        let api_key = live_openai_compatible_api_key(profile).unwrap_or_else(|| {
            panic!(
                "JCODE_ISSUE_DRIVEN_LIVE_PROVIDER={} requires {} or {}",
                provider_id, resolved.api_key_env, resolved.env_file
            )
        });
        let requested_model = std::env::var("JCODE_ISSUE_DRIVEN_LIVE_MODEL")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| resolved.default_model.clone());
        let spend_smoke = env_truthy("JCODE_ISSUE_DRIVEN_LIVE_COMPLETION_SMOKE");
        let stream_smoke = env_truthy("JCODE_ISSUE_DRIVEN_LIVE_STREAM_SMOKE");

        let mut stages = vec![
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_CREDENTIAL_LOADED,
            )
            .with_evidence(
                "auth_source",
                serde_json::json!(api_key.auth.source.clone()),
            )
            .with_evidence("env_key", serde_json::json!(api_key.auth.env_key.clone())),
            cost_quota_safety_stage(spend_smoke || stream_smoke),
        ];

        let models_result = fetch_live_openai_compatible_models(profile, &api_key.secret).await;
        let models = match models_result {
            Ok(models) => models,
            Err(error) => {
                stages.push(crate::live_tests::LiveVerificationStage::failed(
                    crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
                    error.to_string(),
                ));
                let capabilities = covered_stage_names(&stages);
                let event = live_event(
                    "issue_driven_openai_compatible_live_target_smoke",
                    profile,
                    api_key.auth.clone(),
                    requested_model.as_deref(),
                    capabilities,
                    crate::live_tests::LiveVerificationResult::Failed,
                )
                .with_stages(stages);
                append_live_event(&event);
                panic!("live {} model catalog: {error:?}", resolved.display_name);
            }
        };
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_CATALOG_LIVE_ENDPOINT,
            )
            .with_evidence(
                "models",
                crate::live_tests::concise_model_sample(&models, 16),
            ),
        );

        let selected = requested_model
            .filter(|requested| models.iter().any(|model| model == requested))
            .or_else(|| models.first().cloned())
            .expect("live catalog should have at least one model");
        assert!(
            models.iter().any(|model| model == &selected),
            "live {} catalog did not include selected issue target model `{selected}`",
            resolved.display_name
        );

        let driver = AuthLifecycleDriver::new().expect("driver");
        let mut spec = AuthLifecycleSpec::openai_compatible_fixture(
            profile,
            AuthLifecycleAuthPath::RemoteTuiPasteApiKey,
        );
        spec.api_key = api_key.secret.clone();
        spec.catalog_models_after_auth = models.clone();
        spec.selected_model_override = Some(selected.clone());
        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("issue-driven live auth lifecycle fixture");
        result.assert_success(&spec);

        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::AUTH_UX_KEY_ENTRY,
            )
            .with_evidence("auth_path", serde_json::json!("remote_tui_paste_api_key"))
            .with_evidence("simulated_in_sandbox", serde_json::json!(true))
            .with_evidence("transcript_order_verified", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CREDENTIAL_PERSISTENCE,
            )
            .with_evidence(
                "credential_location",
                serde_json::json!(result.credential_location.clone()),
            )
            .with_evidence("sandboxed", serde_json::json!(true)),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::CATALOG_HOT_RELOAD_CURRENT_SESSION,
            )
            .with_evidence("transcript_markers_verified", serde_json::json!(true))
            .with_evidence(
                "catalog_route_count",
                serde_json::json!(result.catalog_routes.len()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_LIVE_MODELS,
            )
            .with_evidence("selected_model", serde_json::json!(selected.clone()))
            .with_evidence(
                "picker_entries",
                serde_json::json!(result.picker.provider_entries.clone()),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::PICKER_FALLBACK_LABELING,
            )
            .with_evidence("fallback_routes_present", serde_json::json!(false))
            .with_evidence(
                "all_picker_entries_from_live_catalog",
                serde_json::json!(true),
            ),
        );
        stages.push(
            crate::live_tests::LiveVerificationStage::passed(
                crate::live_tests::checkpoints::MODEL_SWITCH_ROUTE,
            )
            .with_evidence(
                "switch_request",
                serde_json::json!(result.picker.switch_request.clone()),
            )
            .with_evidence(
                "switch_route_api_method",
                serde_json::json!(result.picker.switch_route_api_method.clone()),
            ),
        );

        if spend_smoke {
            match run_live_openai_compatible_smoke(profile, &api_key.secret, &selected).await {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    stages.push(crate::live_tests::LiveVerificationStage::failed(
                        crate::live_tests::checkpoints::NON_STREAMING_CHAT_COMPLETION,
                        error.to_string(),
                    ));
                    let capabilities = covered_stage_names(&stages);
                    let event = live_event(
                        "issue_driven_openai_compatible_live_target_smoke",
                        profile,
                        api_key.auth.clone(),
                        Some(&selected),
                        capabilities,
                        crate::live_tests::LiveVerificationResult::Failed,
                    )
                    .with_stages(stages);
                    append_live_event(&event);
                    panic!("live {} smoke completion: {error:?}", resolved.display_name);
                }
            }
        }

        if stream_smoke {
            match run_live_openai_compatible_stream_smoke(profile, &api_key.secret, &selected).await
            {
                Ok(stage) => stages.push(stage),
                Err(error) => {
                    stages.push(crate::live_tests::LiveVerificationStage::failed(
                        crate::live_tests::checkpoints::STREAMING_CHAT_COMPLETION,
                        error.to_string(),
                    ));
                    let capabilities = covered_stage_names(&stages);
                    let event = live_event(
                        "issue_driven_openai_compatible_live_target_smoke",
                        profile,
                        api_key.auth.clone(),
                        Some(&selected),
                        capabilities,
                        crate::live_tests::LiveVerificationResult::Failed,
                    )
                    .with_stages(stages);
                    append_live_event(&event);
                    panic!("live {} stream smoke: {error:?}", resolved.display_name);
                }
            }
        }

        let capabilities = covered_stage_names(&stages);
        let event = live_event(
            "issue_driven_openai_compatible_live_target_smoke",
            profile,
            api_key.auth.clone(),
            Some(&selected),
            capabilities,
            crate::live_tests::LiveVerificationResult::Passed,
        )
        .with_stages(stages)
        .with_metadata("issue_driven_provider", serde_json::json!(provider_id));
        append_live_event(&event);
    }

    #[test]
    fn fresh_start_sandbox_is_unconfigured_then_tui_key_lifecycle_configures_provider() {
        let driver = AuthLifecycleDriver::new().expect("driver");
        let spec = AuthLifecycleSpec::cerebras_fixture(AuthLifecycleAuthPath::TuiPasteApiKey);
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(spec.profile);
        let env_file = driver.sandbox.env_file_path(&resolved.env_file);
        let provider = crate::provider_catalog::resolve_login_provider(spec.provider_id)
            .expect("Cerebras login provider descriptor");

        assert!(
            !env_file.exists(),
            "fresh sandbox should not start with a provider env file: {}",
            env_file.display()
        );
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            ),
            None,
            "fresh sandbox should not inherit credentials from the developer machine"
        );
        assert!(
            !crate::provider_catalog::openai_compatible_profile_is_configured(spec.profile),
            "fresh sandbox should report the provider as unconfigured before setup"
        );
        crate::auth::AuthStatus::invalidate_cache();
        assert_eq!(
            crate::auth::AuthStatus::check_fast().state_for_provider(provider),
            crate::auth::AuthState::NotConfigured
        );

        let result = driver
            .run_openai_compatible_fixture(&spec)
            .expect("fresh-start TUI paste-key lifecycle");

        result.assert_success(&spec);
        assert!(
            env_file.exists(),
            "TUI paste-key lifecycle should create env file"
        );
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .as_deref(),
            Some(spec.api_key.as_str())
        );
        assert!(
            result
                .transcript_text()
                .contains("**Cerebras API key saved.**"),
            "fresh-start lifecycle should show the user that the key was saved: {}",
            result.transcript_text()
        );
        crate::auth::AuthStatus::invalidate_cache();
        assert_eq!(
            crate::auth::AuthStatus::check_fast().state_for_provider(provider),
            crate::auth::AuthState::Available
        );
    }
}
