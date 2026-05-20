use anyhow::Result;
use serde::Serialize;
use std::time::Duration;

use crate::cli::provider_init::{self, ProviderChoice};

const AUTH_DOCTOR_VALIDATION_TIMEOUT_SECS: u64 = 120;

#[derive(Debug, Serialize)]
struct AuthStatusProviderReport {
    id: String,
    display_name: String,
    status: String,
    method: String,
    health: String,
    credential_source: String,
    expiry_confidence: String,
    refresh_support: String,
    validation_method: String,
    last_refresh: Option<String>,
    validation: Option<String>,
    auth_kind: String,
    recommended: bool,
}

#[derive(Debug, Serialize)]
struct AuthStatusReport {
    any_available: bool,
    providers: Vec<AuthStatusProviderReport>,
}

#[derive(Debug, Serialize)]
struct AuthDoctorProviderReport {
    id: String,
    display_name: String,
    auth_kind: String,
    recommended: bool,
    status: String,
    method: String,
    health: String,
    credential_source: String,
    credential_source_detail: String,
    expiry_confidence: String,
    refresh_support: String,
    validation_method: String,
    last_refresh: Option<String>,
    last_refresh_detail: Option<AuthDoctorRefreshDetail>,
    validation: Option<String>,
    validation_detail: Option<AuthDoctorValidationDetail>,
    validation_result: Option<String>,
    diagnostics: Vec<String>,
    needs_attention: bool,
    recommended_actions: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuthDoctorRefreshDetail {
    last_attempt_ms: i64,
    last_success_ms: Option<i64>,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthDoctorValidationDetail {
    checked_at_ms: i64,
    success: bool,
    provider_smoke_ok: Option<bool>,
    tool_smoke_ok: Option<bool>,
    stale: bool,
    summary: String,
}

#[derive(Debug, Serialize)]
struct AuthDoctorReport {
    checked_provider: Option<String>,
    validate: bool,
    any_issue: bool,
    providers: Vec<AuthDoctorProviderReport>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProviderListEntry {
    pub(super) id: String,
    pub(super) display_name: String,
    pub(super) auth_kind: Option<String>,
    pub(super) recommended: bool,
    pub(super) aliases: Vec<String>,
    pub(super) detail: Option<String>,
}

#[derive(Debug, Serialize)]
struct ProviderListReport {
    providers: Vec<ProviderListEntry>,
}

#[derive(Debug, Serialize)]
struct ProviderCurrentReport {
    requested_provider: String,
    requested_model: Option<String>,
    resolved_provider: String,
    selected_model: String,
}

#[derive(Debug, Serialize)]
pub(super) struct VersionReport {
    pub(super) version: String,
    pub(super) semver: String,
    pub(super) base_semver: String,
    pub(super) update_semver: String,
    pub(super) git_hash: String,
    pub(super) git_tag: String,
    pub(super) build_time: String,
    pub(super) git_date: String,
    pub(super) release_build: bool,
}

#[derive(Debug, Serialize)]
struct UsageLimitReport {
    name: String,
    usage_percent: f32,
    resets_at: Option<String>,
    reset_in: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageProviderReport {
    provider_name: String,
    limits: Vec<UsageLimitReport>,
    extra_info: Vec<(String, String)>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsageReport {
    providers: Vec<UsageProviderReport>,
}

pub(super) fn run_auth_status_command(emit_json: bool) -> Result<()> {
    let report = build_auth_status_report();
    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in report.providers {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                provider.id,
                provider.status,
                provider.auth_kind,
                provider.method,
                provider.health,
                provider.validation.as_deref().unwrap_or("not validated")
            );
        }
    }

    Ok(())
}

fn build_auth_status_report() -> AuthStatusReport {
    let status = crate::auth::AuthStatus::check();
    let validation = crate::auth::validation::load_all();
    let providers = crate::provider_catalog::auth_status_login_providers();
    let reports = providers
        .into_iter()
        .map(|provider| {
            let assessment = status.assessment_for_provider(provider);
            AuthStatusProviderReport {
                id: provider.id.to_string(),
                display_name: provider.display_name.to_string(),
                status: auth_state_label(assessment.state).to_string(),
                method: assessment.method_detail.clone(),
                health: assessment.health_summary(),
                credential_source: assessment.credential_source.label().to_string(),
                expiry_confidence: assessment.expiry_confidence.label().to_string(),
                refresh_support: assessment.refresh_support.label().to_string(),
                validation_method: assessment.validation_method.label().to_string(),
                last_refresh: assessment
                    .last_refresh
                    .as_ref()
                    .map(crate::auth::refresh_state::format_record_label),
                validation: validation
                    .get(provider.id)
                    .map(crate::auth::validation::format_record_label),
                auth_kind: provider.auth_kind.label().to_string(),
                recommended: provider.recommended,
            }
        })
        .collect::<Vec<_>>();

    AuthStatusReport {
        any_available: status.has_any_available(),
        providers: reports,
    }
}

pub(super) async fn run_auth_doctor_command(
    provider_arg: Option<&str>,
    validate: bool,
    emit_json: bool,
) -> Result<()> {
    let report = build_auth_doctor_report(provider_arg, validate).await?;

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    for (index, provider) in report.providers.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("{} ({})", provider.display_name, provider.id);
        println!("auth_kind: {}", provider.auth_kind);
        println!("status: {}", provider.status);
        println!("method: {}", provider.method);
        println!("health: {}", provider.health);
        println!(
            "credential_source: {} ({})",
            provider.credential_source, provider.credential_source_detail
        );
        println!("expiry: {}", provider.expiry_confidence);
        println!("refresh: {}", provider.refresh_support);
        println!("validation_method: {}", provider.validation_method);
        println!(
            "last_refresh: {}",
            provider.last_refresh.as_deref().unwrap_or("not recorded")
        );
        println!(
            "validation: {}",
            provider.validation.as_deref().unwrap_or("not validated")
        );
        if let Some(validation_result) = provider.validation_result.as_deref() {
            println!("validation_run: {}", validation_result);
        }
        println!("needs_attention: {}", provider.needs_attention);
        if !provider.diagnostics.is_empty() {
            println!("diagnostics:");
            for diagnostic in &provider.diagnostics {
                println!("- {}", diagnostic);
            }
        }
        if !provider.recommended_actions.is_empty() {
            println!("next_steps:");
            for action in &provider.recommended_actions {
                println!("- {}", action);
            }
        }
    }

    Ok(())
}

async fn build_auth_doctor_report(
    provider_arg: Option<&str>,
    validate: bool,
) -> Result<AuthDoctorReport> {
    let mut status = crate::auth::AuthStatus::check();
    let providers = select_auth_doctor_providers(provider_arg, &status)?;
    let mut reports = Vec::new();

    for provider in providers {
        let pre_validation_assessment = status.assessment_for_provider(provider);
        let validation_result = if validate && pre_validation_assessment.is_configured() {
            Some(run_auth_doctor_validation(provider).await)
        } else {
            None
        };
        if validation_result.is_some() {
            crate::auth::AuthStatus::invalidate_cache();
            status = crate::auth::AuthStatus::check();
        }
        let assessment = status.assessment_for_provider(provider);
        let validation = assessment
            .last_validation
            .as_ref()
            .map(crate::auth::validation::format_record_label);
        let validation_detail = assessment
            .last_validation
            .as_ref()
            .map(auth_doctor_validation_detail);
        let last_refresh = assessment
            .last_refresh
            .as_ref()
            .map(crate::auth::refresh_state::format_record_label);
        let last_refresh_detail =
            assessment
                .last_refresh
                .as_ref()
                .map(|record| AuthDoctorRefreshDetail {
                    last_attempt_ms: record.last_attempt_ms,
                    last_success_ms: record.last_success_ms,
                    last_error: record.last_error.clone(),
                });
        let recommended_actions = crate::auth::doctor::recommended_actions(
            provider,
            &assessment,
            validation_result.as_deref(),
        );
        let diagnostics =
            crate::auth::doctor::diagnostics(provider, &assessment, validation_result.as_deref());
        let method = assessment.method_detail.clone();
        let health = assessment.health_summary();
        let needs_attention =
            crate::auth::doctor::needs_attention(&assessment, validation_result.as_deref());

        reports.push(AuthDoctorProviderReport {
            id: provider.id.to_string(),
            display_name: provider.display_name.to_string(),
            auth_kind: provider.auth_kind.label().to_string(),
            recommended: provider.recommended,
            status: auth_state_label(assessment.state).to_string(),
            method,
            health,
            credential_source: assessment.credential_source.label().to_string(),
            credential_source_detail: assessment.credential_source_detail.clone(),
            expiry_confidence: assessment.expiry_confidence.label().to_string(),
            refresh_support: assessment.refresh_support.label().to_string(),
            validation_method: assessment.validation_method.label().to_string(),
            last_refresh,
            last_refresh_detail,
            validation,
            validation_detail,
            validation_result,
            diagnostics,
            needs_attention,
            recommended_actions,
        });
    }

    Ok(AuthDoctorReport {
        checked_provider: provider_arg.map(str::to_string),
        validate,
        any_issue: reports.iter().any(|provider| provider.needs_attention),
        providers: reports,
    })
}

async fn run_auth_doctor_validation(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> String {
    match tokio::time::timeout(
        Duration::from_secs(AUTH_DOCTOR_VALIDATION_TIMEOUT_SECS),
        super::super::auth_test::run_post_login_validation_quiet(provider),
    )
    .await
    {
        Ok(Ok(())) => "validation passed".to_string(),
        Ok(Err(err)) => err.to_string(),
        Err(_) => format!(
            "validation timed out after {}s; run `jcode auth-test --provider {}` for detailed output",
            AUTH_DOCTOR_VALIDATION_TIMEOUT_SECS, provider.id
        ),
    }
}

fn auth_doctor_validation_detail(
    record: &crate::auth::validation::ProviderValidationRecord,
) -> AuthDoctorValidationDetail {
    AuthDoctorValidationDetail {
        checked_at_ms: record.checked_at_ms,
        success: record.success,
        provider_smoke_ok: record.provider_smoke_ok,
        tool_smoke_ok: record.tool_smoke_ok,
        stale: crate::auth::doctor::validation_is_stale(record.checked_at_ms),
        summary: record.summary.clone(),
    }
}

pub(super) fn run_provider_list_command(emit_json: bool) -> Result<()> {
    let providers = list_cli_providers();

    if emit_json {
        let report = ProviderListReport { providers };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for provider in providers {
            if let Some(detail) = provider.detail.as_deref() {
                println!("{}\t{}\t{}", provider.id, provider.display_name, detail);
            } else {
                println!("{}\t{}", provider.id, provider.display_name);
            }
        }
    }

    Ok(())
}

pub(super) async fn run_provider_current_command(
    choice: &ProviderChoice,
    model: Option<&str>,
    emit_json: bool,
) -> Result<()> {
    let provider = provider_init::init_provider_quiet(choice, model).await?;
    let report = ProviderCurrentReport {
        requested_provider: choice.as_arg_value().to_string(),
        requested_model: model.map(str::to_string),
        resolved_provider: crate::provider_catalog::runtime_provider_display_name(provider.name()),
        selected_model: provider.model(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("requested_provider\t{}", report.requested_provider);
        if let Some(requested_model) = report.requested_model.as_deref() {
            println!("requested_model\t{}", requested_model);
        }
        println!("resolved_provider\t{}", report.resolved_provider);
        println!("selected_model\t{}", report.selected_model);
    }

    Ok(())
}

pub(super) fn run_version_command(emit_json: bool) -> Result<()> {
    let report = VersionReport {
        version: env!("JCODE_VERSION").to_string(),
        semver: env!("JCODE_SEMVER").to_string(),
        base_semver: env!("JCODE_BASE_SEMVER").to_string(),
        update_semver: env!("JCODE_UPDATE_SEMVER").to_string(),
        git_hash: env!("JCODE_GIT_HASH").to_string(),
        git_tag: env!("JCODE_GIT_TAG").to_string(),
        build_time: crate::build::current_binary_build_time_string()
            .unwrap_or_else(|| "unknown".to_string()),
        git_date: env!("JCODE_GIT_DATE").to_string(),
        release_build: option_env!("JCODE_RELEASE_BUILD").is_some(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("version\t{}", report.version);
        println!("semver\t{}", report.semver);
        println!("base_semver\t{}", report.base_semver);
        println!("update_semver\t{}", report.update_semver);
        println!("git_hash\t{}", report.git_hash);
        println!("git_tag\t{}", report.git_tag);
        println!("build_time\t{}", report.build_time);
        println!("git_date\t{}", report.git_date);
        println!("release_build\t{}", report.release_build);
    }

    Ok(())
}

pub(super) async fn run_usage_command(emit_json: bool) -> Result<()> {
    let providers = crate::usage::fetch_all_provider_usage().await;

    let report = UsageReport {
        providers: providers.iter().map(usage_provider_report).collect(),
    };

    if emit_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if report.providers.is_empty() {
        println!("No connected providers");
        println!();
        println!("Next steps:");
        println!("- Use `jcode login --provider claude` to connect Claude OAuth.");
        println!("- Use `jcode login --provider openai` to connect ChatGPT / Codex OAuth.");
        return Ok(());
    }

    for (idx, provider) in report.providers.iter().enumerate() {
        if idx > 0 {
            println!();
        }

        println!("{}", provider.provider_name);
        println!("{}", "-".repeat(provider.provider_name.chars().count()));

        if let Some(error) = &provider.error {
            println!("error: {}", error);
            continue;
        }

        if provider.limits.is_empty() && provider.extra_info.is_empty() {
            println!("No usage data available.");
            continue;
        }

        for limit in &provider.limits {
            match limit.reset_in.as_deref() {
                Some(reset_in) => println!(
                    "{}: {} (resets in {})",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15),
                    reset_in
                ),
                None => println!(
                    "{}: {}",
                    limit.name,
                    crate::usage::format_usage_bar(limit.usage_percent, 15)
                ),
            }
        }

        if !provider.extra_info.is_empty() {
            if !provider.limits.is_empty() {
                println!();
            }
            for (key, value) in &provider.extra_info {
                println!("{}: {}", key, value);
            }
        }
    }

    Ok(())
}

fn select_auth_doctor_providers(
    provider_arg: Option<&str>,
    status: &crate::auth::AuthStatus,
) -> Result<Vec<crate::provider_catalog::LoginProviderDescriptor>> {
    if let Some(provider_arg) = provider_arg {
        let provider =
            crate::provider_catalog::resolve_login_provider(provider_arg).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown provider '{}'. Use `jcode provider list` to see valid provider ids.",
                    provider_arg
                )
            })?;
        return Ok(vec![provider]);
    }

    let configured = crate::provider_catalog::auth_status_login_providers()
        .into_iter()
        .filter(|provider| status.assessment_for_provider(*provider).is_configured())
        .collect::<Vec<_>>();
    if configured.is_empty() {
        Ok(crate::provider_catalog::auth_status_login_providers().to_vec())
    } else {
        Ok(configured)
    }
}

fn usage_provider_report(provider: &crate::usage::ProviderUsage) -> UsageProviderReport {
    UsageProviderReport {
        provider_name: provider.provider_name.clone(),
        limits: provider
            .limits
            .iter()
            .map(|limit| UsageLimitReport {
                name: limit.name.clone(),
                usage_percent: limit.usage_percent,
                resets_at: limit.resets_at.clone(),
                reset_in: limit
                    .resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time),
            })
            .collect(),
        extra_info: provider.extra_info.clone(),
        error: provider.error.clone(),
    }
}

pub(super) fn list_cli_providers() -> Vec<ProviderListEntry> {
    let choices = [
        ProviderChoice::Jcode,
        ProviderChoice::Claude,
        ProviderChoice::Openai,
        ProviderChoice::Openrouter,
        ProviderChoice::Azure,
        ProviderChoice::Opencode,
        ProviderChoice::OpencodeGo,
        ProviderChoice::Zai,
        ProviderChoice::Kimi,
        ProviderChoice::Groq,
        ProviderChoice::Mistral,
        ProviderChoice::Perplexity,
        ProviderChoice::TogetherAi,
        ProviderChoice::Deepinfra,
        ProviderChoice::Xai,
        ProviderChoice::Chutes,
        ProviderChoice::Cerebras,
        ProviderChoice::AlibabaCodingPlan,
        ProviderChoice::OpenaiCompatible,
        ProviderChoice::Cursor,
        ProviderChoice::Copilot,
        ProviderChoice::Gemini,
        ProviderChoice::Antigravity,
        ProviderChoice::Google,
        ProviderChoice::Auto,
    ];

    choices
        .into_iter()
        .map(|choice| {
            if let Some(provider) = provider_init::login_provider_for_choice(&choice) {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: provider.display_name.to_string(),
                    auth_kind: Some(provider.auth_kind.label().to_string()),
                    recommended: provider.recommended,
                    aliases: provider
                        .aliases
                        .iter()
                        .map(|alias| (*alias).to_string())
                        .collect(),
                    detail: Some(provider.menu_detail.to_string()),
                }
            } else {
                ProviderListEntry {
                    id: choice.as_arg_value().to_string(),
                    display_name: "Auto-detect".to_string(),
                    auth_kind: None,
                    recommended: false,
                    aliases: Vec::new(),
                    detail: Some("Use the best configured provider automatically".to_string()),
                }
            }
        })
        .collect()
}

fn auth_state_label(state: crate::auth::AuthState) -> &'static str {
    match state {
        crate::auth::AuthState::Available => "available",
        crate::auth::AuthState::Expired => "expired",
        crate::auth::AuthState::NotConfigured => "not_configured",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_status<'a>(
        report: &'a AuthStatusReport,
        provider_id: &str,
    ) -> &'a AuthStatusProviderReport {
        report
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .unwrap_or_else(|| panic!("missing auth status provider `{}`", provider_id))
    }

    fn provider_doctor<'a>(
        report: &'a AuthDoctorReport,
        provider_id: &str,
    ) -> &'a AuthDoctorProviderReport {
        report
            .providers
            .iter()
            .find(|provider| provider.id == provider_id)
            .unwrap_or_else(|| panic!("missing auth doctor provider `{}`", provider_id))
    }

    #[tokio::test]
    async fn cli_auth_status_doctor_and_login_lifecycle_uses_fresh_sandbox() {
        let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().expect("sandbox");
        let provider = crate::provider_catalog::CEREBRAS_LOGIN_PROVIDER;
        let profile = crate::provider_catalog::CEREBRAS_PROFILE;
        let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
        let env_file = sandbox.env_file_path(&resolved.env_file);

        assert!(
            !env_file.exists(),
            "fresh CLI sandbox should start without {}",
            env_file.display()
        );
        crate::auth::AuthStatus::invalidate_cache();

        let before_status = build_auth_status_report();
        let before_cerebras = provider_status(&before_status, provider.id);
        assert_eq!(before_cerebras.status, "not_configured");
        assert_eq!(before_cerebras.auth_kind, "API key");
        assert_eq!(before_cerebras.credential_source, "none");
        assert_eq!(before_cerebras.method, "not configured");

        let before_doctor = build_auth_doctor_report(Some(provider.id), false)
            .await
            .expect("doctor before login");
        assert_eq!(before_doctor.checked_provider.as_deref(), Some(provider.id));
        assert!(before_doctor.any_issue);
        let before_doctor_provider = provider_doctor(&before_doctor, provider.id);
        assert_eq!(before_doctor_provider.status, "not_configured");
        assert!(before_doctor_provider.needs_attention);
        assert!(before_doctor_provider.diagnostics.iter().any(|line| {
            line == &format!("{} is not configured for jcode yet.", provider.display_name)
        }));
        assert!(
            before_doctor_provider
                .recommended_actions
                .iter()
                .any(|line| {
                    line == &format!("Connect it: `jcode login --provider {}`", provider.id)
                })
        );

        crate::cli::login::run_login(
            &crate::cli::provider_init::ProviderChoice::Cerebras,
            None,
            crate::cli::login::LoginOptions {
                no_validate: true,
                openai_compatible_api_key: Some("test-cerebras-cli-key".to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CLI login should save Cerebras key in sandbox");

        assert!(
            env_file.exists(),
            "CLI login should create provider env file"
        );
        assert_eq!(
            crate::provider_catalog::load_api_key_from_env_or_config(
                &resolved.api_key_env,
                &resolved.env_file,
            )
            .as_deref(),
            Some("test-cerebras-cli-key")
        );
        crate::env::remove_var(&resolved.api_key_env);
        crate::auth::AuthStatus::invalidate_cache();

        let after_status = build_auth_status_report();
        let after_cerebras = provider_status(&after_status, provider.id);
        assert!(after_status.any_available);
        assert_eq!(after_cerebras.status, "available");
        assert_eq!(after_cerebras.auth_kind, "API key");
        assert_eq!(after_cerebras.credential_source, "app config file");
        assert!(after_cerebras.method.contains(&resolved.api_key_env));
        assert!(
            after_cerebras.health.contains(&resolved.env_file),
            "status should show the sandbox env-file-backed source detail: {:?}",
            after_cerebras
        );

        let after_doctor = build_auth_doctor_report(Some(provider.id), false)
            .await
            .expect("doctor after login");
        assert_eq!(after_doctor.checked_provider.as_deref(), Some(provider.id));
        let after_doctor_provider = provider_doctor(&after_doctor, provider.id);
        assert_eq!(after_doctor_provider.status, "available");
        assert_eq!(after_doctor_provider.credential_source, "app config file");
        assert!(after_doctor_provider.needs_attention);
        assert!(
            after_doctor_provider
                .diagnostics
                .iter()
                .any(|line| { line == "No runtime validation has been recorded." })
        );
        assert!(
            after_doctor_provider
                .recommended_actions
                .iter()
                .any(|line| {
                    line == &format!(
                        "Run runtime verification: `jcode auth-test --provider {}`",
                        provider.id
                    )
                })
        );
        assert!(
            after_doctor_provider
                .recommended_actions
                .iter()
                .any(|line| { line == "Review current state: `jcode auth status --json`" })
        );
    }
}
