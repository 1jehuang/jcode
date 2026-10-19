use super::{
    MACRO_BRANCHES, MAX_MICRO_CANDIDATES, ScoredSuggestion, SuggestedRepliesContext,
    SuggestedRepliesDebugInfo, SuggestedRepliesResponse,
    candidates::{
        build_macro_branch_choice_question_with_options,
        build_macro_branch_noul_questions_with_options, build_micro_scoring_questions,
        build_structured_systemone_state, contextual_candidates_for_branch_with_options,
        merge_custom_prompts, score_locally_with_context_and_options,
    },
    distribute_suggestions, push_unique_case_insensitive,
};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct OpenGfRawOutput {
    #[serde(default)]
    branch: String,
    #[serde(default)]
    candidates: Vec<String>,
}

/// Parse OpenGF (`Sidecar`) JSON or line-based response into `(branch, candidates)`.
pub fn parse_open_gf_response(raw: &str) -> Option<(String, Vec<String>)> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let json_slice = if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        &trimmed[start..=end]
    } else {
        trimmed
    };
    if let Ok(parsed) = serde_json::from_str::<OpenGfRawOutput>(json_slice) {
        let candidates: Vec<String> = parsed
            .candidates
            .into_iter()
            .map(|c| c.trim().to_string())
            .filter(|c| !c.is_empty())
            .take(MAX_MICRO_CANDIDATES)
            .collect();
        if !candidates.is_empty() {
            let branch = if parsed.branch.trim().is_empty() {
                "implementation".to_string()
            } else {
                parsed.branch.trim().to_ascii_lowercase()
            };
            return Some((branch, candidates));
        }
    }
    None
}

fn open_gf_http_client(is_plain_http: bool) -> Result<&'static reqwest::Client> {
    static HTTP_NO_PROXY_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    static HTTPS_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

    if is_plain_http {
        if let Some(c) = HTTP_NO_PROXY_CLIENT.get() {
            return Ok(c);
        }
        let built = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(4))
            .timeout(std::time::Duration::from_secs(12))
            .no_proxy()
            .build()
            .context("failed to build OpenGF HTTP no_proxy client")?;
        drop(HTTP_NO_PROXY_CLIENT.set(built));
        HTTP_NO_PROXY_CLIENT
            .get()
            .context("OpenGF HTTP no_proxy client not initialized")
    } else {
        if let Some(c) = HTTPS_CLIENT.get() {
            return Ok(c);
        }
        let built = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(4))
            .timeout(std::time::Duration::from_secs(12))
            .build()
            .context("failed to build OpenGF HTTPS client")?;
        drop(HTTPS_CLIENT.set(built));
        HTTPS_CLIENT
            .get()
            .context("OpenGF HTTPS client not initialized")
    }
}

pub(super) async fn complete_via_named_openai_compatible(
    name: &str,
    profile: &crate::config::NamedProviderConfig,
    system: &str,
    user_msg: &str,
) -> Result<(String, String)> {
    ensure!(
        matches!(
            profile.provider_type,
            crate::config::NamedProviderType::OpenAiCompatible
                | crate::config::NamedProviderType::OpenRouter
        ),
        "profile '{name}' uses unsupported provider type {:?} for OpenAI-compatible chat completions",
        profile.provider_type
    );
    if let Some(api) = profile
        .api
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let api_lower = api.to_ascii_lowercase();
        ensure!(
            matches!(
                api_lower.as_str(),
                "openai"
                    | "openai-completions"
                    | "openai-compatible"
                    | "chat-completions"
                    | "openrouter"
            ),
            "profile '{name}' uses unsupported api '{api}'"
        );
    }

    let base = profile.base_url.trim().trim_end_matches('/');
    ensure!(!base.is_empty(), "profile '{name}' has empty base_url");
    let url = if base.ends_with("/chat/completions") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    };

    let key = profile
        .api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            let env_key = profile.api_key_env.as_deref()?.trim();
            if env_key.is_empty() {
                return None;
            }
            if env_key.starts_with("sk-") {
                return Some(env_key.to_string());
            }
            if let Some(env_file) = profile
                .env_file
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                && let Some(v) =
                    crate::provider_catalog::load_env_value_from_env_or_config(env_key, env_file)
            {
                let sanitized = jcode_provider_env::sanitize_secret_value(&v);
                if !sanitized.is_empty() {
                    return Some(sanitized.to_string());
                }
            }
            match std::env::var(env_key) {
                Ok(v) => Some(v.trim().to_string()).filter(|v| !v.is_empty()),
                Err(_) => None,
            }
        });

    let model = profile
        .default_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            profile
                .models
                .first()
                .map(|m| m.id.trim())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("gpt-5.6-luna");

    let mut payload = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_msg }
        ],
        "max_tokens": 900,
        "temperature": 0.2,
        "stream": false
    });
    if let Some(extra) = &profile.extra_body {
        let extra_obj = extra
            .as_object()
            .with_context(|| format!("profile '{name}' extra_body must be a JSON object"))?;
        if let Some(obj) = payload.as_object_mut() {
            for (k, v) in extra_obj {
                obj.insert(k.clone(), v.clone());
            }
        }
    }

    let client = open_gf_http_client(url.starts_with("http://"))?;
    let mut req = client.post(&url).json(&payload);
    match profile.auth {
        crate::config::NamedProviderAuth::None => {}
        crate::config::NamedProviderAuth::Bearer => {
            if let Some(token) = key {
                req = req.bearer_auth(token);
            } else if profile.requires_api_key != Some(false) {
                anyhow::bail!("profile '{name}' requires a bearer API key");
            }
        }
        crate::config::NamedProviderAuth::Header => {
            let header_name = profile
                .auth_header
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("api-key");
            if let Some(token) = key {
                req = req.header(header_name, token);
            } else if profile.requires_api_key != Some(false) {
                anyhow::bail!("profile '{name}' requires an API key for header '{header_name}'");
            }
        }
    }
    for (hk, hv) in &profile.headers {
        req = req.header(hk, hv);
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("POST {url} failed"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .with_context(|| format!("invalid JSON from {url} (status {status})"))?;
    if !status.is_success() {
        anyhow::bail!("HTTP {status} from {name} ({model}): {body}");
    }
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    ensure!(
        !content.is_empty(),
        "empty choices[0].message.content from {name} ({model})"
    );
    Ok((content, format!("profile:{name}:{model}")))
}

async fn complete_open_gf_prompt(system: &str, user_msg: &str) -> Result<(String, String)> {
    let mut errors = Vec::new();

    // 1. Primary Sidecar (OpenAI gpt-5.6-luna / Claude Haiku / registered active provider).
    let sidecar = crate::sidecar::Sidecar::new();
    let sidecar_label = format!(
        "sidecar:{}:{}",
        sidecar.backend_name(),
        sidecar.model_name()
    );
    match sidecar.complete(system, user_msg).await {
        Ok(raw) if !raw.trim().is_empty() => return Ok((raw, sidecar_label)),
        Ok(_) => errors.push(format!("{sidecar_label} returned empty output")),
        Err(err) => errors.push(format!("{sidecar_label}: {err}")),
    }

    // 2. Live registered active provider fork (if Sidecar had picked expired OpenAI/Claude creds).
    if sidecar.backend_name() != "provider"
        && let Some(provider) = crate::provider::active_provider_fork()
    {
        let label = format!("active:{}:{}", provider.name(), provider.model());
        match provider.complete_simple_with_usage(user_msg, system).await {
            Ok((raw, _)) if !raw.trim().is_empty() => return Ok((raw, label)),
            Ok(_) => errors.push(format!("{label} returned empty output")),
            Err(err) => errors.push(format!("{label}: {err}")),
        }
    }

    // 3. Configured named providers in config.toml (starts with default_provider, e.g. clipapi/dragon,
    // which makes OpenGF work inside the Remote TUI client process where active_provider_fork is None).
    let cfg = crate::config::config();
    let mut named_candidates: Vec<(String, crate::config::NamedProviderConfig)> = Vec::new();
    if let Some(default_prov) = cfg.provider.default_provider.as_deref()
        && let Some(profile_cfg) = cfg.providers.get(default_prov)
    {
        named_candidates.push((default_prov.to_string(), profile_cfg.clone()));
    }
    for (name, profile_cfg) in &cfg.providers {
        if !named_candidates.iter().any(|(n, _)| n == name) {
            named_candidates.push((name.clone(), profile_cfg.clone()));
        }
    }
    for (name, profile_cfg) in named_candidates {
        match complete_via_named_openai_compatible(&name, &profile_cfg, system, user_msg).await {
            Ok(res) => return Ok(res),
            Err(err) => errors.push(format!("profile:{name}: {err}")),
        }
    }

    // 4. Registered external OAuth provider runtimes (Antigravity / Gemini) if available.
    for runtime_key in [
        crate::provider::external::ANTIGRAVITY_RUNTIME,
        crate::provider::external::GEMINI_RUNTIME,
    ] {
        if let Some(provider) =
            crate::provider::external::instantiate_external_provider(runtime_key)
        {
            let label = format!("external:{runtime_key}:{}", provider.model());
            match provider.complete_simple_with_usage(user_msg, system).await {
                Ok((raw, _)) if !raw.trim().is_empty() => return Ok((raw, label)),
                Ok(_) => errors.push(format!("{label} returned empty output")),
                Err(err) => errors.push(format!("{label}: {err}")),
            }
        }
    }

    anyhow::bail!("All OpenGF routes failed: {}", errors.join(" | "))
}

/// Synthesize context-specific next-step candidates via OpenGF using structured `SuggestedRepliesContext`.
pub async fn generate_open_gf_candidates_for_context(
    ctx: &SuggestedRepliesContext,
    desired_count: usize,
) -> Result<(String, Vec<String>, String)> {
    let target_count = desired_count.clamp(4, MAX_MICRO_CANDIDATES);
    let system = "You are OpenGF, the generative candidate synthesizer for Jcode's two-stage SystemOne Suggested Replies cascade. Return ONLY valid JSON with the shape: {\"branch\": \"feature_completion|goal_delivery|planning|implementation|verification|debugging|review|architecture|refactoring\", \"candidates\": [\"short actionable reply 1\", \"short actionable reply 2\", ...]}. Generate concise, complete, self-contained next-step user replies (up to 120 chars each, never cut mid-sentence) that move the user toward achieving the successful final completion of the feature and goal, tailored to the conversation state, extracted assistant options, and pending todos. CRITICAL: Write every candidate in the EXACT SAME natural language used by the user in their most recent message (e.g. Russian if the user wrote in Russian, English if in English). Order candidates from the most likely immediate next user reply (#1) to lower-priority alternatives.";

    let mut prompt_sections = Vec::new();
    prompt_sections.push(format!(
        "Detected user language: {}",
        if ctx.language_is_cyrillic {
            "Russian (Cyrillic)"
        } else {
            "English"
        }
    ));
    if !ctx.assistant_options.is_empty() {
        prompt_sections.push(format!(
            "Assistant options / questions to answer directly:\n- {}",
            ctx.assistant_options.join("\n- ")
        ));
    }
    if !ctx.pending_todos.is_empty() {
        prompt_sections.push(format!(
            "Active pending todos:\n- {}",
            ctx.pending_todos.join("\n- ")
        ));
    }
    prompt_sections.push(format!(
        "Recent conversation state:\n{}",
        crate::util::truncate_str(&ctx.transcript_tail, 3200)
    ));
    prompt_sections.push(format!(
        "Produce the JSON with macro branch and 4..{target_count} concrete next-step user replies in the user's language:"
    ));
    let user_msg = prompt_sections.join("\n\n");

    let (raw, route_label) = complete_open_gf_prompt(system, &user_msg).await?;
    let (branch, candidates) = parse_open_gf_response(&raw).with_context(|| {
        format!("OpenGF response from {route_label} did not contain valid JSON candidates")
    })?;
    Ok((branch, candidates, route_label))
}

/// Synthesize context-specific next-step candidates via OpenGF (`Sidecar` + fallback routes),
/// returning `(branch, candidates, route_label)`.
pub async fn generate_open_gf_candidates_with_route(
    transcript_tail: &str,
) -> Result<(String, Vec<String>, String)> {
    let ctx = SuggestedRepliesContext::from_transcript(transcript_tail);
    generate_open_gf_candidates_for_context(&ctx, 6).await
}

/// Synthesize context-specific next-step candidates via OpenGF (`Sidecar`).
pub async fn generate_open_gf_candidates(transcript_tail: &str) -> Result<(String, Vec<String>)> {
    let (branch, candidates, _) = generate_open_gf_candidates_with_route(transcript_tail).await?;
    Ok((branch, candidates))
}

/// Calibrate SystemOne `noul` probability and optional comparative `choice` softmax
/// probability with candidate rank prior when the model positively endorses the candidate.
pub fn calibrate_micro_score_with_choice(
    raw_noul: f32,
    choice_prob: Option<f32>,
    is_choice_winner: bool,
    idx: usize,
    from_open_gf: bool,
) -> f32 {
    let raw = if raw_noul.is_finite() {
        raw_noul.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if raw < 0.50 && !is_choice_winner {
        return raw;
    }
    let rank_prior: f32 = match idx {
        0 => 0.88,
        1 => 0.74,
        2 => 0.64,
        3 => 0.56,
        4 => 0.48,
        _ => (0.42 - (idx.saturating_sub(5) as f32) * 0.015).max(0.20),
    };
    let bonus = if from_open_gf && raw >= 0.55 && idx == 0 {
        0.06
    } else if raw >= 0.58 && idx == 0 {
        0.05
    } else {
        0.0
    };
    let choice_boost = choice_prob
        .filter(|p| p.is_finite() && *p > 0.0)
        .map(|p| (p.clamp(0.0, 1.0) * 0.14) + if is_choice_winner { 0.04 } else { 0.0 })
        .unwrap_or(if is_choice_winner { 0.05 } else { 0.0 });

    (0.60 * raw + 0.40 * rank_prior + bonus + choice_boost).clamp(0.0, 1.0)
}

/// Backward-compatible calibration helper when only `noul` is present.
pub fn calibrate_micro_score(raw_noul: f32, idx: usize, from_open_gf: bool) -> f32 {
    calibrate_micro_score_with_choice(raw_noul, None, false, idx, from_open_gf)
}

/// Execute the full two-stage cascade using a structured `SuggestedRepliesContext`
/// (Macro-Branching -> Candidate Merging -> Joint `choice + noul` Micro-Scoring -> Distribution).
pub async fn evaluate_cascade_with_context(
    ctx: &SuggestedRepliesContext,
    config: &crate::config::SuggestedRepliesConfig,
    six_lens_findings: Option<&str>,
) -> SuggestedRepliesResponse {
    let started = std::time::Instant::now();
    let threshold = config.clamped_confidence_threshold();
    let max_secondary = config.clamped_max_secondary();
    let custom_prompts = config.sanitized_custom_prompts();
    let desired_pool = (max_secondary + 1).clamp(6, MAX_MICRO_CANDIDATES);

    if !config.enabled {
        return SuggestedRepliesResponse::default();
    }

    // If 6-lens review findings were just reported, prioritize surfacing them.
    if six_lens_findings.is_some_and(|s| !s.trim().is_empty()) {
        let mut resp = score_locally_with_context_and_options(
            ctx,
            None,
            &custom_prompts,
            six_lens_findings,
            threshold,
            max_secondary,
            config.enable_finish_category,
        );
        resp.debug = Some(SuggestedRepliesDebugInfo {
            source: "six_lens".to_string(),
            open_gf_status: "skipped (6-lens findings present)".to_string(),
            jev_status: "skipped (6-lens findings present)".to_string(),
            threshold,
            top_confidence: resp.top_confidence(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
        return resp;
    }

    // Stage 1A: If use_open_gf is enabled, synthesize dynamic candidates via Sidecar / fallback routes.
    let mut open_gf_status = "disabled (use_open_gf=false)".to_string();
    let open_gf_result = if config.use_open_gf {
        let gf_start = std::time::Instant::now();
        match generate_open_gf_candidates_for_context(ctx, desired_pool).await {
            Ok((branch, cands, route)) => {
                let ms = gf_start.elapsed().as_millis();
                open_gf_status = format!(
                    "ok ({route}, branch={branch}, {} candidates, {ms}ms)",
                    cands.len()
                );
                crate::logging::info(&format!(
                    "[suggested_replies] OpenGF synthesized {} candidates via {route} in {ms}ms (branch={branch})",
                    cands.len()
                ));
                Some((branch, cands))
            }
            Err(err) => {
                let ms = gf_start.elapsed().as_millis();
                open_gf_status = format!("error ({ms}ms): {err}");
                crate::logging::warn(&format!(
                    "[suggested_replies] OpenGF synthesis failed after {ms}ms: {err}"
                ));
                None
            }
        }
    } else {
        None
    };

    let mut jev_status = format!("unavailable (selector={})", config.jev_provider);

    // Build ordered list of JevClients to attempt:
    // If `jev_provider == "auto"` and the primary cloud provider (e.g. OpenJev) fails at runtime
    // (such as HTTP 402), automatically fall back to local `Laya` (`http://127.0.0.1:8080/v1/systemone`)
    // when `laya.env` / `JCODE_LAYA_BASE_URL` / `LAYA_BASE_URL` / `LAYA_API_KEY` is configured.
    let mut clients_to_try: Vec<(String, crate::jev::JevClient)> = Vec::new();
    match crate::jev::JevClient::for_suggestions_with_selector(&config.jev_provider) {
        Ok(primary) => {
            let effective_selector = match std::env::var("JCODE_SUGGESTIONS_JEV_PROVIDER") {
                Ok(v) if !v.trim().is_empty() => v,
                _ => config.jev_provider.clone(),
            };
            let is_auto = effective_selector.trim().eq_ignore_ascii_case("auto");
            let primary_is_laya = primary.provider_name() == "laya";
            clients_to_try.push((
                format!("{}:{}", effective_selector.trim(), primary.provider_name()),
                primary,
            ));
            if is_auto
                && !primary_is_laya
                && crate::jev::JevClient::has_configured_laya_route()
                && let Ok(laya_client) =
                    crate::jev::JevClient::for_suggestions_fixed_selector("laya")
            {
                clients_to_try.push(("auto->laya".to_string(), laya_client));
            }
        }
        Err(err) => {
            jev_status = format!("client init error: {err}");
            crate::logging::info(&format!(
                "[suggested_replies] JevClient unavailable ({}), using local scorer",
                err
            ));
        }
    }

    for (selector_label, client) in clients_to_try {
        let iter1_state = build_structured_systemone_state(None, ctx, &[]);

        // Iteration 1: Determine macro branch (from OpenGF if present, else via SystemOne).
        let branch_res: Result<String, String> = if let Some((ref gf_branch, _)) = open_gf_result {
            Ok(gf_branch.clone())
        } else if client.supports_choice_questions() {
            let q = build_macro_branch_choice_question_with_options(config.enable_finish_category);
            match client.evaluate(iter1_state, q).await {
                Ok(resp) => Ok(resp
                    .pointer("/answers/macro_branch/choice")
                    .and_then(Value::as_str)
                    .unwrap_or("implementation")
                    .to_string()),
                Err(err) => Err(format!("iter1(choice,{selector_label}) error: {err}")),
            }
        } else {
            let q = build_macro_branch_noul_questions_with_options(config.enable_finish_category);
            match client.evaluate(iter1_state, q).await {
                Ok(resp) => Ok(MACRO_BRANCHES
                    .iter()
                    .filter(|(id, _)| config.enable_finish_category || *id != "feature_completion")
                    .max_by(|(a, _), (b, _)| {
                        let pa = resp
                            .pointer(&format!("/answers/branch_{a}/noul"))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0);
                        let pb = resp
                            .pointer(&format!("/answers/branch_{b}/noul"))
                            .and_then(Value::as_f64)
                            .unwrap_or(0.0);
                        pa.total_cmp(&pb)
                    })
                    .map(|(id, _)| (*id).to_string())
                    .unwrap_or_else(|| "implementation".to_string())),
                Err(err) => Err(format!("iter1(noul,{selector_label}) error: {err}")),
            }
        };

        let raw_branch = match branch_res {
            Ok(b) => b,
            Err(err_msg) => {
                jev_status = err_msg.clone();
                crate::logging::info(&format!(
                    "[suggested_replies] {err_msg}; trying next route or local scorer"
                ));
                continue;
            }
        };
        let branch = if !config.enable_finish_category
            && matches!(
                raw_branch.trim().to_ascii_lowercase().as_str(),
                "finish" | "feature_completion" | "final_completion"
            ) {
            "goal_delivery".to_string()
        } else {
            raw_branch
        };

        // Iteration 2: Build candidate pool (OpenGF or contextual + custom_prompts)
        // and evaluate via joint `choice + noul` SystemOne request.
        let from_open_gf = open_gf_result
            .as_ref()
            .is_some_and(|(_, cands)| !cands.is_empty());
        let contextual_pool = contextual_candidates_for_branch_with_options(
            &branch,
            ctx,
            config.enable_finish_category,
        );
        let base_candidates = match open_gf_result.as_ref() {
            Some((_, cands)) if !cands.is_empty() => {
                let mut combined = cands.clone();
                for extra in contextual_pool {
                    if combined.len() >= desired_pool {
                        break;
                    }
                    push_unique_case_insensitive(&mut combined, extra);
                }
                combined
            }
            _ => contextual_pool,
        };
        let candidate_texts = merge_custom_prompts(base_candidates, &custom_prompts, desired_pool);
        let include_choice = client.supports_choice_questions() && candidate_texts.len() >= 2;

        if let Ok(micro_q) =
            build_micro_scoring_questions(&branch, &candidate_texts, include_choice)
        {
            let iter2_state =
                build_structured_systemone_state(Some(&branch), ctx, &candidate_texts);
            let iter2_start = std::time::Instant::now();
            match client.evaluate(iter2_state, micro_q).await {
                Ok(resp) => {
                    let iter2_ms = iter2_start.elapsed().as_millis();
                    let choice_winner = resp
                        .pointer("/answers/best_candidate/choice")
                        .and_then(Value::as_str);
                    let scored: Vec<ScoredSuggestion> = candidate_texts
                        .into_iter()
                        .enumerate()
                        .map(|(idx, text)| {
                            let cand_key = format!("cand_{idx}");
                            let raw_prob = resp
                                .pointer(&format!("/answers/{cand_key}/noul"))
                                .and_then(Value::as_f64)
                                .unwrap_or(0.0) as f32;
                            let choice_prob = resp
                                .pointer(&format!(
                                    "/answers/best_candidate/probabilities/{cand_key}"
                                ))
                                .and_then(Value::as_f64)
                                .map(|p| p as f32);
                            let is_winner = choice_winner == Some(cand_key.as_str());
                            let confidence = calibrate_micro_score_with_choice(
                                raw_prob,
                                choice_prob,
                                is_winner,
                                idx,
                                from_open_gf,
                            );
                            ScoredSuggestion { text, confidence }
                        })
                        .collect();
                    let mut distributed =
                        distribute_suggestions(branch.clone(), scored, threshold, max_secondary);
                    distributed.populate_goal_categories_with_options(
                        &custom_prompts,
                        config.enable_finish_category,
                    );
                    let top_conf = distributed.top_confidence();
                    jev_status = format!(
                        "ok (selector={selector_label}, mode={}, branch={branch}, iter2={}ms, top={:.2})",
                        if include_choice {
                            "choice+noul"
                        } else {
                            "noul"
                        },
                        iter2_ms,
                        top_conf.unwrap_or(0.0)
                    );
                    crate::logging::info(&format!(
                        "[suggested_replies] Cascade complete in {}ms: source={}, branch={branch}, top_conf={:?}, threshold={threshold:.2}, default_action={:?}",
                        started.elapsed().as_millis(),
                        if from_open_gf {
                            "opengf+jev"
                        } else {
                            "fixed+jev"
                        },
                        top_conf,
                        distributed.default_action
                    ));
                    distributed.debug = Some(SuggestedRepliesDebugInfo {
                        source: if from_open_gf {
                            "opengf+jev".to_string()
                        } else {
                            "fixed+jev".to_string()
                        },
                        open_gf_status,
                        jev_status,
                        threshold,
                        top_confidence: top_conf,
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                    return distributed;
                }
                Err(err) => {
                    jev_status = format!("iter2({selector_label}) error: {err}");
                    crate::logging::warn(&format!(
                        "[suggested_replies] Iteration 2 ({selector_label}) failed: {err}"
                    ));
                }
            }
        }
    }

    let from_open_gf = open_gf_result
        .as_ref()
        .is_some_and(|(_, cands)| !cands.is_empty());
    let mut fallback = score_locally_with_context_and_options(
        ctx,
        open_gf_result,
        &custom_prompts,
        six_lens_findings,
        threshold,
        max_secondary,
        config.enable_finish_category,
    );
    fallback.debug = Some(SuggestedRepliesDebugInfo {
        source: if from_open_gf {
            "opengf+local".to_string()
        } else {
            "fixed+local".to_string()
        },
        open_gf_status,
        jev_status,
        threshold,
        top_confidence: fallback.top_confidence(),
        elapsed_ms: started.elapsed().as_millis() as u64,
    });
    fallback
}

/// Execute the full two-stage cascade (backward-compatible entry point taking `transcript_tail`).
pub async fn evaluate_cascade(
    transcript_tail: &str,
    config: &crate::config::SuggestedRepliesConfig,
    six_lens_findings: Option<&str>,
) -> SuggestedRepliesResponse {
    let ctx = SuggestedRepliesContext::from_transcript(transcript_tail);
    evaluate_cascade_with_context(&ctx, config, six_lens_findings).await
}
