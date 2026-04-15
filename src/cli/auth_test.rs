use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

const DEFAULT_AUTH_TEST_PROVIDER_PROMPT: &str =
    "Reply with exactly AUTH_TEST_OK and nothing else. Do not call tools.";
const DEFAULT_AUTH_TEST_TOOL_PROMPT: &str = "If tools are available, use exactly one trivial tool call and then reply with exactly AUTH_TEST_OK and nothing else.";

#[expect(
    clippy::large_enum_variant,
    reason = "Generic auth-test targets carry provider descriptors until this CLI path is refactored"
)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolvedAuthTestTarget {
    Detailed(AuthTestTarget),
    Generic {
        provider: crate::provider_catalog::LoginProviderDescriptor,
        choice: super::provider_init::ProviderChoice,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthTestTarget {
    Claude,
    Openai,
    Gemini,
    Antigravity,
    Google,
    Copilot,
    Cursor,
}

impl AuthTestTarget {
    fn provider_choice(self) -> super::provider_init::ProviderChoice {
        match self {
            Self::Claude => super::provider_init::ProviderChoice::Claude,
            Self::Openai => super::provider_init::ProviderChoice::Openai,
            Self::Gemini => super::provider_init::ProviderChoice::Gemini,
            Self::Antigravity => super::provider_init::ProviderChoice::Antigravity,
            Self::Google => super::provider_init::ProviderChoice::Google,
            Self::Copilot => super::provider_init::ProviderChoice::Copilot,
            Self::Cursor => super::provider_init::ProviderChoice::Cursor,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Openai => "openai",
            Self::Gemini => "gemini",
            Self::Antigravity => "antigravity",
            Self::Google => "google",
            Self::Copilot => "copilot",
            Self::Cursor => "cursor",
        }
    }

    fn supports_smoke(self) -> bool {
        !matches!(self, Self::Google)
    }

    fn from_provider_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        match choice {
            super::provider_init::ProviderChoice::Claude
            | super::provider_init::ProviderChoice::ClaudeSubprocess => Some(Self::Claude),
            super::provider_init::ProviderChoice::Openai => Some(Self::Openai),
            super::provider_init::ProviderChoice::Gemini => Some(Self::Gemini),
            super::provider_init::ProviderChoice::Antigravity => Some(Self::Antigravity),
            super::provider_init::ProviderChoice::Google => Some(Self::Google),
            super::provider_init::ProviderChoice::Copilot => Some(Self::Copilot),
            super::provider_init::ProviderChoice::Cursor => Some(Self::Cursor),
            _ => None,
        }
    }

    fn credential_paths(self) -> Result<Vec<String>> {
        match self {
            Self::Claude => Ok(vec![
                crate::auth::claude::jcode_path()?.display().to_string(),
                crate::storage::user_home_path(".claude/.credentials.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Openai => Ok(vec![
                crate::storage::jcode_dir()?
                    .join("openai-auth.json")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".codex/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Gemini => Ok(vec![
                crate::auth::gemini::tokens_path()?.display().to_string(),
                crate::auth::gemini::gemini_cli_oauth_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Antigravity => Ok(vec![
                crate::auth::antigravity::tokens_path()?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Google => Ok(vec![
                crate::auth::google::credentials_path()?
                    .display()
                    .to_string(),
                crate::auth::google::tokens_path()?.display().to_string(),
            ]),
            Self::Copilot => Ok(vec![
                crate::storage::user_home_path(".copilot/config.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/hosts.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/github-copilot/apps.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".local/share/opencode/auth.json")?
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".pi/agent/auth.json")?
                    .display()
                    .to_string(),
            ]),
            Self::Cursor => Ok(vec![
                dirs::config_dir()
                    .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
                    .join("jcode")
                    .join("cursor.env")
                    .display()
                    .to_string(),
                crate::storage::user_home_path(".config/Cursor/User/globalStorage/state.vscdb")?
                    .display()
                    .to_string(),
            ]),
        }
    }
}

#[derive(Debug, Serialize)]
struct AuthTestStepReport {
    name: String,
    ok: bool,
    detail: String,
}

#[derive(Debug, Serialize)]
struct AuthTestProviderReport {
    provider: String,
    credential_paths: Vec<String>,
    steps: Vec<AuthTestStepReport>,
    smoke_output: Option<String>,
    tool_smoke_output: Option<String>,
    success: bool,
}

impl AuthTestProviderReport {
    fn new(target: AuthTestTarget) -> Self {
        Self {
            provider: target.label().to_string(),
            credential_paths: target.credential_paths().unwrap_or_default(),
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn new_generic(provider_id: String, credential_paths: Vec<String>) -> Self {
        Self {
            provider: provider_id,
            credential_paths,
            steps: Vec::new(),
            smoke_output: None,
            tool_smoke_output: None,
            success: true,
        }
    }

    fn push_step(&mut self, name: impl Into<String>, ok: bool, detail: impl Into<String>) {
        if !ok {
            self.success = false;
        }
        self.steps.push(AuthTestStepReport {
            name: name.into(),
            ok,
            detail: detail.into(),
        });
    }
}

impl ResolvedAuthTestTarget {
    fn from_choice(choice: &super::provider_init::ProviderChoice) -> Option<Self> {
        let provider = super::provider_init::login_provider_for_choice(choice)?;
        Some(match AuthTestTarget::from_provider_choice(choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic {
                provider,
                choice: choice.clone(),
            },
        })
    }

    fn from_provider(provider: crate::provider_catalog::LoginProviderDescriptor) -> Option<Self> {
        let choice = super::provider_init::choice_for_login_provider(provider)?;
        Some(match AuthTestTarget::from_provider_choice(&choice) {
            Some(target) => Self::Detailed(target),
            None => Self::Generic { provider, choice },
        })
    }
}

#[derive(Clone, Copy)]
enum AuthTestSmokeKind {
    Provider,
    Tool,
}

impl AuthTestSmokeKind {
    fn step_name(self) -> &'static str {
        match self {
            Self::Provider => "provider_smoke",
            Self::Tool => "tool_smoke",
        }
    }

    fn skipped_by_flag_detail(self) -> &'static str {
        match self {
            Self::Provider => "Skipped by --no-smoke.",
            Self::Tool => "Skipped by --no-tool-smoke.",
        }
    }

    fn unsupported_detail(self) -> &'static str {
        "Skipped: provider is auth/tool-only and has no model runtime smoke step."
    }

    fn success_detail(self) -> &'static str {
        match self {
            Self::Provider => "Provider returned AUTH_TEST_OK.",
            Self::Tool => "Tool-enabled provider request returned AUTH_TEST_OK.",
        }
    }

    fn failure_detail(self, output: &str) -> String {
        match self {
            Self::Provider => {
                format!("Provider response did not contain AUTH_TEST_OK: {}", output)
            }
            Self::Tool => format!(
                "Tool-enabled provider response did not contain AUTH_TEST_OK: {}",
                output
            ),
        }
    }

    async fn run(
        self,
        target: AuthTestTarget,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        self.run_for_choice(&target.provider_choice(), model, prompt)
            .await
    }

    async fn run_for_choice(
        self,
        choice: &super::provider_init::ProviderChoice,
        model: Option<&str>,
        prompt: &str,
    ) -> Result<String> {
        match self {
            Self::Provider => run_provider_smoke_for_choice(choice, model, prompt).await,
            Self::Tool => run_provider_tool_smoke_for_choice(choice, model, prompt).await,
        }
    }

    fn set_output(self, report: &mut AuthTestProviderReport, output: String) {
        match self {
            Self::Provider => report.smoke_output = Some(output),
            Self::Tool => report.tool_smoke_output = Some(output),
        }
    }
}

fn push_result_step<T, E, F>(
    report: &mut AuthTestProviderReport,
    name: &'static str,
    result: std::result::Result<T, E>,
    detail: F,
) -> Option<T>
where
    E: std::fmt::Display,
    F: FnOnce(&T) -> String,
{
    match result {
        Ok(value) => {
            report.push_step(name, true, detail(&value));
            Some(value)
        }
        Err(err) => {
            report.push_step(name, false, err.to_string());
            None
        }
    }
}

fn auth_email_suffix(email: Option<&str>) -> String {
    email
        .map(|email| format!(" for {}", email))
        .unwrap_or_default()
}

async fn maybe_run_auth_test_smoke(
    report: &mut AuthTestProviderReport,
    kind: AuthTestSmokeKind,
    target: AuthTestTarget,
    model: Option<&str>,
    enabled: bool,
    prompt: &str,
) {
    if enabled && report.success && target.supports_smoke() {
        match kind.run(target, model, prompt).await {
            Ok(output) => {
                let ok = output.contains("AUTH_TEST_OK");
                kind.set_output(report, output.clone());
                report.push_step(
                    kind.step_name(),
                    ok,
                    if ok {
                        kind.success_detail().to_string()
                    } else {
                        kind.failure_detail(&output)
                    },
                );
            }
            Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
        }
    } else if !target.supports_smoke() {
        report.push_step(kind.step_name(), true, kind.unsupported_detail());
    } else if !enabled {
        report.push_step(kind.step_name(), true, kind.skipped_by_flag_detail());
    }
}

async fn maybe_run_auth_test_smoke_for_choice(
    report: &mut AuthTestProviderReport,
    kind: AuthTestSmokeKind,
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    enabled: bool,
    prompt: &str,
) {
    if enabled && report.success {
        match auth_test_choice_plan(choice, model).await {
            Ok(AuthTestChoicePlan::Run { model }) => {
                match kind.run_for_choice(choice, model.as_deref(), prompt).await {
                    Ok(output) => {
                        let ok = output.contains("AUTH_TEST_OK");
                        kind.set_output(report, output.clone());
                        report.push_step(
                            kind.step_name(),
                            ok,
                            if ok {
                                kind.success_detail().to_string()
                            } else {
                                kind.failure_detail(&output)
                            },
                        );
                    }
                    Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
                }
            }
            Ok(AuthTestChoicePlan::Skip(detail)) => {
                report.push_step(kind.step_name(), true, detail);
            }
            Err(err) => report.push_step(kind.step_name(), false, format!("{err:#}")),
        }
    } else if !enabled {
        report.push_step(kind.step_name(), true, kind.skipped_by_flag_detail());
    }
}

pub(crate) async fn run_post_login_validation(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Result<()> {
    let Some(choice) = super::provider_init::choice_for_login_provider(provider) else {
        eprintln!(
            "\nSkipping automatic runtime validation for {}. Auto Import can add multiple providers; run `jcode auth-test --all-configured` to validate them.",
            provider.display_name
        );
        return Ok(());
    };

    super::provider_init::apply_login_provider_profile_env(provider);

    eprintln!(
        "\nValidating {} login with live auth/runtime checks...",
        provider.display_name
    );

    let report = if let Some(target) = AuthTestTarget::from_provider_choice(&choice) {
        populate_auth_test_target_report(
            target,
            None,
            true,
            true,
            DEFAULT_AUTH_TEST_PROVIDER_PROMPT,
            DEFAULT_AUTH_TEST_TOOL_PROMPT,
            AuthTestProviderReport::new(target),
        )
        .await
    } else {
        populate_generic_auth_test_report(
            provider,
            choice.clone(),
            None,
            true,
            true,
            DEFAULT_AUTH_TEST_PROVIDER_PROMPT,
            DEFAULT_AUTH_TEST_TOOL_PROMPT,
            AuthTestProviderReport::new_generic(
                choice.as_arg_value().to_string(),
                generic_credential_paths_for_provider(provider),
            ),
        )
        .await
    };

    persist_auth_test_report(&report);
    print_auth_test_reports(std::slice::from_ref(&report));

    if report.success {
        Ok(())
    } else if AuthTestTarget::from_provider_choice(&choice).is_some() {
        anyhow::bail!(
            "Post-login validation failed for {}. Credentials were saved, but jcode could not verify runtime readiness. Re-run `jcode auth-test --provider {}` for details.",
            provider.display_name,
            choice.as_arg_value()
        )
    } else {
        anyhow::bail!(
            "Post-login validation failed for {}. Credentials were saved, but jcode could not verify runtime readiness. Re-test with `jcode --provider {} run \"Reply with exactly AUTH_TEST_OK and nothing else.\"` after fixing the provider/runtime.",
            provider.display_name,
            choice.as_arg_value()
        )
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "CLI auth-test entrypoint maps directly from command-line flags"
)]
pub async fn run_auth_test_command(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    login: bool,
    all_configured: bool,
    no_smoke: bool,
    no_tool_smoke: bool,
    prompt: Option<&str>,
    emit_json: bool,
    output_path: Option<&str>,
) -> Result<()> {
    let targets = resolve_auth_test_targets(choice, all_configured)?;
    let provider_smoke_prompt = prompt.unwrap_or(DEFAULT_AUTH_TEST_PROVIDER_PROMPT);
    let tool_smoke_prompt = prompt.unwrap_or(DEFAULT_AUTH_TEST_TOOL_PROMPT);

    let mut reports = Vec::new();
    for target in targets {
        let report = match target {
            ResolvedAuthTestTarget::Detailed(target) => {
                run_auth_test_target(
                    target,
                    model,
                    login,
                    !no_smoke,
                    !no_tool_smoke,
                    provider_smoke_prompt,
                    tool_smoke_prompt,
                )
                .await
            }
            ResolvedAuthTestTarget::Generic { provider, choice } => {
                let mut report = AuthTestProviderReport::new_generic(
                    choice.as_arg_value().to_string(),
                    generic_credential_paths_for_provider(provider),
                );
                if login {
                    match super::login::run_login(
                        &choice,
                        None,
                        super::login::LoginOptions::default(),
                    )
                    .await
                    {
                        Ok(()) => report.push_step("login", true, "Login flow completed."),
                        Err(err) => report.push_step("login", false, err.to_string()),
                    }
                }
                populate_generic_auth_test_report(
                    provider,
                    choice,
                    model,
                    !no_smoke,
                    !no_tool_smoke,
                    provider_smoke_prompt,
                    tool_smoke_prompt,
                    report,
                )
                .await
            }
        };
        persist_auth_test_report(&report);
        reports.push(report);
    }

    let report_json = (emit_json || output_path.is_some())
        .then(|| serde_json::to_string_pretty(&reports))
        .transpose()?;

    if let Some(path) = output_path {
        std::fs::write(path, report_json.as_deref().unwrap_or("[]"))
            .with_context(|| format!("failed to write auth-test report to {}", path))?;
    }

    if emit_json {
        println!("{}", report_json.as_deref().unwrap_or("[]"));
    } else {
        print_auth_test_reports(&reports);
    }

    if reports.iter().all(|report| report.success) {
        Ok(())
    } else {
        anyhow::bail!("One or more auth tests failed")
    }
}

pub(crate) fn resolve_auth_test_targets(
    choice: &super::provider_init::ProviderChoice,
    all_configured: bool,
) -> Result<Vec<ResolvedAuthTestTarget>> {
    if all_configured || matches!(choice, super::provider_init::ProviderChoice::Auto) {
        let status = crate::auth::AuthStatus::check();
        let targets = configured_auth_test_targets(&status);
        if targets.is_empty() {
            anyhow::bail!(
                "No configured supported auth providers found. Run `jcode login --provider <provider>` first, or choose an explicit --provider."
            );
        }
        return Ok(targets);
    }

    ResolvedAuthTestTarget::from_choice(choice)
        .map(|target| vec![target])
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Provider '{}' is not yet supported by `jcode auth-test`.",
                choice.as_arg_value()
            )
        })
}

pub(crate) fn configured_auth_test_targets(
    status: &crate::auth::AuthStatus,
) -> Vec<ResolvedAuthTestTarget> {
    crate::provider_catalog::auth_status_login_providers()
        .into_iter()
        .filter(|provider| {
            status.state_for_provider(*provider) != crate::auth::AuthState::NotConfigured
        })
        .filter_map(ResolvedAuthTestTarget::from_provider)
        .collect()
}

async fn run_auth_test_target(
    target: AuthTestTarget,
    model: Option<&str>,
    login: bool,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
) -> AuthTestProviderReport {
    let mut report = AuthTestProviderReport::new(target);

    if login {
        match super::login::run_login(
            &target.provider_choice(),
            None,
            super::login::LoginOptions::default(),
        )
        .await
        {
            Ok(()) => report.push_step("login", true, "Login flow completed."),
            Err(err) => report.push_step("login", false, err.to_string()),
        }
    }

    populate_auth_test_target_report(
        target,
        model,
        run_smoke,
        run_tool_smoke,
        provider_smoke_prompt,
        tool_smoke_prompt,
        report,
    )
    .await
}

async fn populate_auth_test_target_report(
    target: AuthTestTarget,
    model: Option<&str>,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
    mut report: AuthTestProviderReport,
) -> AuthTestProviderReport {
    match target {
        AuthTestTarget::Claude => probe_claude_auth(&mut report).await,
        AuthTestTarget::Openai => probe_openai_auth(&mut report).await,
        AuthTestTarget::Gemini => probe_gemini_auth(&mut report).await,
        AuthTestTarget::Antigravity => probe_antigravity_auth(&mut report).await,
        AuthTestTarget::Google => probe_google_auth(&mut report).await,
        AuthTestTarget::Copilot => probe_copilot_auth(&mut report).await,
        AuthTestTarget::Cursor => probe_cursor_auth(&mut report).await,
    }

    maybe_run_auth_test_smoke(
        &mut report,
        AuthTestSmokeKind::Provider,
        target,
        model,
        run_smoke,
        provider_smoke_prompt,
    )
    .await;

    maybe_run_auth_test_smoke(
        &mut report,
        AuthTestSmokeKind::Tool,
        target,
        model,
        run_tool_smoke,
        tool_smoke_prompt,
    )
    .await;

    report
}

#[expect(
    clippy::too_many_arguments,
    reason = "Auth-test helper carries explicit smoke and prompt controls until structured options land"
)]
async fn populate_generic_auth_test_report(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    choice: super::provider_init::ProviderChoice,
    model: Option<&str>,
    run_smoke: bool,
    run_tool_smoke: bool,
    provider_smoke_prompt: &str,
    tool_smoke_prompt: &str,
    mut report: AuthTestProviderReport,
) -> AuthTestProviderReport {
    super::provider_init::apply_login_provider_profile_env(provider);
    probe_generic_provider_auth(provider, &mut report);

    maybe_run_auth_test_smoke_for_choice(
        &mut report,
        AuthTestSmokeKind::Provider,
        &choice,
        model,
        run_smoke,
        provider_smoke_prompt,
    )
    .await;

    maybe_run_auth_test_smoke_for_choice(
        &mut report,
        AuthTestSmokeKind::Tool,
        &choice,
        model,
        run_tool_smoke,
        tool_smoke_prompt,
    )
    .await;

    report
}

fn persist_auth_test_report(report: &AuthTestProviderReport) {
    let step_map = report
        .steps
        .iter()
        .map(|step| (step.name.as_str(), step.ok))
        .collect::<HashMap<_, _>>();
    let summary = report
        .steps
        .iter()
        .find(|step| !step.ok)
        .map(|step| format!("{}: {}", step.name, step.detail))
        .or_else(|| {
            report
                .steps
                .last()
                .map(|step| format!("{}: {}", step.name, step.detail))
        })
        .unwrap_or_else(|| "No validation steps recorded.".to_string());

    let record = crate::auth::validation::ProviderValidationRecord {
        checked_at_ms: chrono::Utc::now().timestamp_millis(),
        success: report.success,
        provider_smoke_ok: step_map.get("provider_smoke").copied(),
        tool_smoke_ok: step_map.get("tool_smoke").copied(),
        summary,
    };

    if let Err(err) = crate::auth::validation::save(&report.provider, record) {
        crate::logging::warn(&format!(
            "failed to persist auth validation result for {}: {}",
            report.provider, err
        ));
    }
}

fn generic_credential_paths_for_provider(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Vec<String> {
    let Ok(config_dir) = crate::storage::app_config_dir() else {
        return Vec::new();
    };

    match provider.target {
        crate::provider_catalog::LoginProviderTarget::Jcode => {
            vec![config_dir.join(crate::subscription_catalog::JCODE_ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenRouter => {
            vec![config_dir.join("openrouter.env")]
        }
        crate::provider_catalog::LoginProviderTarget::Azure => {
            vec![config_dir.join(crate::auth::azure::ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            vec![config_dir.join(resolved.env_file)]
        }
        _ => Vec::new(),
    }
    .into_iter()
    .map(|path| path.display().to_string())
    .collect()
}

fn auth_state_label(state: crate::auth::AuthState) -> &'static str {
    match state {
        crate::auth::AuthState::Available => "available",
        crate::auth::AuthState::Expired => "expired",
        crate::auth::AuthState::NotConfigured => "not_configured",
    }
}

fn probe_generic_provider_auth(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    report: &mut AuthTestProviderReport,
) {
    let status = crate::auth::AuthStatus::check();
    let state = status.state_for_provider(provider);
    let detail = status.method_detail_for_provider(provider);
    report.push_step(
        "credential_probe",
        state == crate::auth::AuthState::Available,
        format!(
            "{} auth status is {} ({detail}).",
            provider.display_name,
            auth_state_label(state),
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: provider does not expose a dedicated refresh probe in jcode today.".to_string(),
    );
}

async fn probe_claude_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::claude::load_credentials(),
        |creds| {
            format!(
                "Loaded Claude credentials (expires_at={}).",
                creds.expires_at
            )
        },
    ) {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await,
            |tokens| {
                format!(
                    "Claude token refresh succeeded (new_expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_openai_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::codex::load_credentials(),
        |creds| {
            if creds.refresh_token.trim().is_empty() {
                "Loaded OpenAI API key credentials (no refresh token present).".to_string()
            } else {
                format!(
                    "Loaded OpenAI OAuth credentials (expires_at={:?}).",
                    creds.expires_at
                )
            }
        },
    ) {
        if creds.refresh_token.trim().is_empty() {
            report.push_step(
                "refresh_probe",
                true,
                "Skipped: OpenAI is using API key auth, not OAuth.",
            );
        } else {
            push_result_step(
                report,
                "refresh_probe",
                crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await,
                |tokens| {
                    format!(
                        "OpenAI token refresh succeeded (new_expires_at={}).",
                        tokens.expires_at
                    )
                },
            );
        }
    }
}

async fn probe_gemini_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::gemini::load_tokens(),
        |tokens| {
            format!(
                "Loaded Gemini tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::gemini::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Gemini token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_antigravity_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::antigravity::load_tokens(),
        |tokens| {
            format!(
                "Loaded Antigravity OAuth tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::antigravity::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Antigravity token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_google_auth(report: &mut AuthTestProviderReport) {
    let creds_result = crate::auth::google::load_credentials();
    let tokens_result = crate::auth::google::load_tokens();
    match (creds_result, tokens_result) {
        (Ok(creds), Ok(tokens)) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded Google credentials (client_id={}...) and Gmail tokens{}.",
                    &creds.client_id[..20.min(creds.client_id.len())],
                    auth_email_suffix(tokens.email.as_deref())
                ),
            );
            match crate::auth::google::get_valid_token().await {
                Ok(_) => report.push_step(
                    "refresh_probe",
                    true,
                    "Google/Gmail token load/refresh succeeded.".to_string(),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        (Err(err), _) => report.push_step("credential_probe", false, err.to_string()),
        (_, Err(err)) => report.push_step("credential_probe", false, err.to_string()),
    }
}

async fn probe_copilot_auth(report: &mut AuthTestProviderReport) {
    if let Some(token) = push_result_step(
        report,
        "credential_probe",
        crate::auth::copilot::load_github_token(),
        |token| {
            format!(
                "Loaded GitHub OAuth token for Copilot ({} chars).",
                token.len()
            )
        },
    ) {
        let client = reqwest::Client::new();
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::copilot::exchange_github_token(&client, &token).await,
            |api_token| {
                format!(
                    "Exchanged GitHub token for Copilot API token (expires_at={}).",
                    api_token.expires_at
                )
            },
        );
    }
}

async fn probe_cursor_auth(report: &mut AuthTestProviderReport) {
    let has_agent_auth = crate::auth::cursor::has_cursor_agent_auth();
    let has_api_key = crate::auth::cursor::has_cursor_api_key();
    let has_vscdb = crate::auth::cursor::has_cursor_vscdb_token();
    let ok = has_agent_auth || has_api_key || has_vscdb;
    report.push_step(
        "credential_probe",
        ok,
        format!(
            "Cursor auth sources: agent_session={}, api_key={}, vscdb_token={}",
            has_agent_auth, has_api_key, has_vscdb
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: Cursor provider does not expose a native refresh-token probe in jcode today."
            .to_string(),
    );
}

#[derive(Debug)]
pub(crate) enum AuthTestChoicePlan {
    Run { model: Option<String> },
    Skip(String),
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiCompatibleModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelInfo {
    id: String,
}

pub(crate) async fn auth_test_choice_plan(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
) -> Result<AuthTestChoicePlan> {
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        return Ok(AuthTestChoicePlan::Run {
            model: Some(model.to_string()),
        });
    }

    let Some(profile) = super::provider_init::profile_for_choice(choice) else {
        return Ok(AuthTestChoicePlan::Run { model: None });
    };
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    if resolved.requires_api_key || resolved.default_model.is_some() {
        return Ok(AuthTestChoicePlan::Run { model: None });
    }

    crate::provider_catalog::apply_openai_compatible_profile_env(Some(profile));
    let discovered_model = discover_openai_compatible_validation_model(&resolved).await?;
    if let Some(model) = discovered_model {
        return Ok(AuthTestChoicePlan::Run { model: Some(model) });
    }

    Ok(AuthTestChoicePlan::Skip(format!(
        "Skipped: {} local endpoint reported no models. Re-run `jcode auth-test --provider {} --model <local-model>` or set a default model first.",
        resolved.display_name,
        choice.as_arg_value()
    )))
}

async fn discover_openai_compatible_validation_model(
    profile: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Result<Option<String>> {
    let url = format!("{}/models", profile.api_base.trim_end_matches('/'));
    let mut request = crate::provider::shared_http_client().get(&url);
    if let Some(api_key) = crate::provider_catalog::load_api_key_from_env_or_config(
        &profile.api_key_env,
        &profile.env_file,
    ) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.with_context(|| {
        format!(
            "Failed to query {} models from {} during auth-test validation",
            profile.display_name, url
        )
    })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "{} model discovery failed (HTTP {}): {}",
            profile.display_name,
            status,
            body.trim()
        );
    }

    let parsed: OpenAiCompatibleModelsResponse =
        serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse {} model discovery response from {}",
                profile.display_name, url
            )
        })?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .find(|model| !model.is_empty()))
}

async fn run_provider_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    run_auth_test_with_retry(async || {
        let provider = super::provider_init::init_provider_for_validation(choice, model)
            .await
            .with_context(|| format!("Failed to initialize {} provider", choice.as_arg_value()))?;
        let output = provider
            .complete_simple(prompt, "")
            .await
            .with_context(|| format!("{} provider smoke prompt failed", choice.as_arg_value()))?;
        Ok(output.trim().to_string())
    })
    .await
}

async fn run_provider_tool_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    use futures::StreamExt;

    run_auth_test_with_retry(async || {
        let (provider, registry) =
            super::provider_init::init_provider_and_registry_for_validation(choice, model)
                .await
                .with_context(|| {
                    format!("Failed to initialize {} provider", choice.as_arg_value())
                })?;
        registry
            .register_mcp_tools(None, None, Some("auth-test".to_string()))
            .await;
        let tools = registry.definitions(None).await;

        let messages = vec![crate::message::Message {
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: prompt.to_string(),
                cache_control: None,
            }],
            timestamp: None,
            tool_duration_ms: None,
        }];

        let response = provider
            .complete(&messages, &tools, "", None)
            .await
            .with_context(|| {
                format!(
                    "{} tool-enabled smoke prompt failed with {} attached tools",
                    choice.as_arg_value(),
                    tools.len()
                )
            })?;

        tokio::pin!(response);
        let mut output = String::new();
        while let Some(event) = response.next().await {
            match event {
                Ok(crate::message::StreamEvent::TextDelta(text)) => output.push_str(&text),
                Ok(_) => {}
                Err(err) => return Err(err),
            }
        }

        Ok(output.trim().to_string())
    })
    .await
}

async fn run_auth_test_with_retry<F, Fut>(mut f: F) -> Result<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    const RETRY_DELAYS: &[Duration] = &[Duration::from_secs(3), Duration::from_secs(8)];

    let mut last_err = None;
    for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
        match f().await {
            Ok(output) => return Ok(output),
            Err(err) if auth_test_error_is_retryable(&err) => {
                last_err = Some(err);
                crate::logging::warn(&format!(
                    "auth-test transient failure on attempt {} - retrying in {}s",
                    attempt + 1,
                    delay.as_secs()
                ));
                tokio::time::sleep(*delay).await;
            }
            Err(err) => return Err(err),
        }
    }

    match f().await {
        Ok(output) => Ok(output),
        Err(err) if last_err.is_some() => Err(err),
        Err(err) => Err(err),
    }
}

pub(crate) fn auth_test_error_is_retryable(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    [
        "http 429",
        "too many requests",
        "resource_exhausted",
        "rate_limit_exceeded",
        "rate limit",
        "temporarily unavailable",
        "timeout",
        "connection reset",
        "service unavailable",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn print_auth_test_reports(reports: &[AuthTestProviderReport]) {
    for report in reports {
        println!("=== auth-test: {} ===", report.provider);
        if !report.credential_paths.is_empty() {
            println!("credential paths:");
            for path in &report.credential_paths {
                println!("  - {}", path);
            }
        }
        for step in &report.steps {
            let marker = if step.ok { "✓" } else { "✗" };
            println!("{} {} — {}", marker, step.name, step.detail);
        }
        if let Some(output) = report.smoke_output.as_deref() {
            println!("smoke output: {}", output);
        }
        if let Some(output) = report.tool_smoke_output.as_deref() {
            println!("tool smoke output: {}", output);
        }
        println!("result: {}\n", if report.success { "PASS" } else { "FAIL" });
    }
}
