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
                if matches!(kind, AuthTestSmokeKind::Tool)
                    && let Some(detail) = tool_smoke_skip_detail_for_choice(choice, model.as_deref())
                {
                    report.push_step(kind.step_name(), true, detail);
                    return;
                }
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
    run_post_login_validation_inner(provider, true).await
}

pub(crate) async fn run_post_login_validation_quiet(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Result<()> {
    run_post_login_validation_inner(provider, false).await
}

async fn run_post_login_validation_inner(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    verbose: bool,
) -> Result<()> {
    let Some(choice) = super::provider_init::choice_for_login_provider(provider) else {
        crate::logging::auth_event(
            "post_login_validation_skipped",
            provider.id,
            &[("reason", "no_runtime_provider_choice")],
        );
        if verbose {
            eprintln!(
                "\nSkipping automatic runtime validation for {}. Auto Import can add multiple providers; run `jcode auth-test --all-configured` to validate them.",
                provider.display_name
            );
        }
        return Ok(());
    };

    super::provider_init::apply_login_provider_profile_env(provider);
    crate::logging::auth_event(
        "post_login_validation_started",
        provider.id,
        &[("choice", choice.as_arg_value())],
    );

    if verbose {
        eprintln!(
            "\nValidating {} login with live auth/runtime checks...",
            provider.display_name
        );
    }

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
            choice,
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

    persist_auth_test_report(&report, None);
    let step_count = report.steps.len().to_string();
    crate::logging::auth_event(
        "post_login_validation_completed",
        provider.id,
        &[
            ("choice", choice.as_arg_value()),
            ("success", if report.success { "true" } else { "false" }),
            ("steps", step_count.as_str()),
        ],
    );
    if verbose {
        print_auth_test_reports(std::slice::from_ref(&report));
    }

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
        persist_auth_test_report(&report, model);
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
        // Auth-test discovery must not run slow or blocking provider-global probes.
        // Generic OpenAI-compatible providers only need local env/config detection,
        // and detailed providers perform their own provider-specific checks later.
        let status = crate::auth::AuthStatus::check_fast();
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
        .filter(|provider| status.assessment_for_provider(*provider).is_configured())
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

fn persist_auth_test_report(report: &AuthTestProviderReport, model: Option<&str>) {
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

    if let Err(err) = persist_auth_test_live_verification_event(report, model) {
        crate::logging::warn(&format!(
            "failed to persist auth-test live verification event for {}: {}",
            report.provider, err
        ));
    }
}

fn persist_auth_test_live_verification_event(
    report: &AuthTestProviderReport,
    model: Option<&str>,
) -> Result<()> {
    let mut stages = Vec::new();
    let mut expected = Vec::new();
    let mut capabilities = Vec::new();

    for step in &report.steps {
        match step.name.as_str() {
            "credential_probe" => {
                expected.push(crate::live_tests::checkpoints::AUTH_CREDENTIAL_LOADED);
                stages.push(auth_test_step_stage(
                    crate::live_tests::checkpoints::AUTH_CREDENTIAL_LOADED,
                    step,
                ));
            }
            "provider_smoke" => {
                capabilities.push("provider_smoke");
            }
            "tool_smoke" => {
                capabilities.push("real_jcode_tool_smoke");
                expected.push(crate::live_tests::checkpoints::TOOL_EXECUTION_LOOP);
                expected.push(crate::live_tests::checkpoints::TOOL_RESULT_FOLLOWUP);
                expected.push(crate::live_tests::checkpoints::REAL_JCODE_TOOL_SMOKE);
                let stage = auth_test_step_stage(
                    crate::live_tests::checkpoints::REAL_JCODE_TOOL_SMOKE,
                    step,
                )
                .with_evidence("tool_name", serde_json::json!(AUTH_TEST_TOOL_NAME))
                .with_evidence("tool_command", serde_json::json!(AUTH_TEST_TOOL_COMMAND));
                stages.push(stage.clone());
                stages.push(auth_test_tool_derived_stage(
                    crate::live_tests::checkpoints::TOOL_EXECUTION_LOOP,
                    step,
                ));
                stages.push(auth_test_tool_derived_stage(
                    crate::live_tests::checkpoints::TOOL_RESULT_FOLLOWUP,
                    step,
                ));
            }
            _ => {}
        }
    }

    if expected.is_empty() {
        return Ok(());
    }

    let result = if report.success {
        crate::live_tests::LiveVerificationResult::Passed
    } else {
        crate::live_tests::LiveVerificationResult::Failed
    };
    let mut event = crate::live_tests::LiveVerificationEvent::new(
        "auth_test_real_jcode_runtime",
        report.provider.clone(),
        report.provider.clone(),
        crate::live_tests::LiveVerificationAuth::non_secret("auth-test", None::<String>),
        result,
    )
    .with_expected_checkpoints(expected)
    .with_capabilities(capabilities)
    .with_stages(stages)
    .with_metadata(
        "checkpoint_taxonomy_version",
        serde_json::json!(crate::live_tests::CHECKPOINT_TAXONOMY_VERSION),
    )
    .with_metadata("auth_test_steps", serde_json::json!(report.steps));
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        event = event.with_model(model.to_string());
    }
    crate::live_tests::append_event(&event)?;
    Ok(())
}

fn auth_test_step_stage(
    checkpoint: &'static str,
    step: &AuthTestStepReport,
) -> crate::live_tests::LiveVerificationStage {
    let status = if step.ok {
        crate::live_tests::LiveVerificationStageStatus::Passed
    } else {
        crate::live_tests::LiveVerificationStageStatus::Failed
    };
    crate::live_tests::LiveVerificationStage::new(checkpoint, status)
        .with_evidence("auth_test_step", serde_json::json!(step.name))
        .with_evidence("detail", serde_json::json!(step.detail))
}

fn auth_test_tool_derived_stage(
    checkpoint: &'static str,
    step: &AuthTestStepReport,
) -> crate::live_tests::LiveVerificationStage {
    auth_test_step_stage(checkpoint, step).with_evidence(
        "derived_from",
        serde_json::json!(crate::live_tests::checkpoints::REAL_JCODE_TOOL_SMOKE),
    )
}
