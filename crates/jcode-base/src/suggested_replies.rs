//! Two-stage cascading Suggested Replies with Default Action Focus.
//!
//! Combines OpenJev SystemOne (`https://api.openjev.sh/v1/systemone`, calibrated
//! logit decisions via `choice` and `noul`) with OpenGF (`Sidecar` generative
//! candidate synthesis with `"store": false`) and a deterministic local fallback.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

/// Macro conversation branches for Iteration 1 classification.
pub const MACRO_BRANCHES: &[(&str, &str)] = &[
    (
        "implementation",
        "Writing, editing, or completing code, features, or configuration changes",
    ),
    (
        "verification",
        "Running tests, compiler checks, benchmarks, or verifying runtime behavior",
    ),
    (
        "debugging",
        "Investigating errors, test failures, panics, or unexpected behavior",
    ),
    (
        "review",
        "Reviewing diffs, auditing code quality/security across 6 lenses, or preparing a PR",
    ),
    (
        "architecture",
        "Exploring design, OpenSpec proposals/specs/tasks, or repository structure",
    ),
    (
        "refactoring",
        "Cleaning up code, simplifying modules, or improving maintainability",
    ),
];

/// Scored suggestion candidate produced by Iteration 2.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredSuggestion {
    pub text: String,
    pub confidence: f32,
}

/// Diagnostic telemetry for the most recent Suggested Replies cascade evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRepliesDebugInfo {
    pub source: String,
    pub open_gf_status: String,
    pub jev_status: String,
    pub threshold: f32,
    pub top_confidence: Option<f32>,
    pub elapsed_ms: u64,
}

/// Final two-stage cascade response matching the contract:
/// `{ defaultAction: string | null, secondarySuggestions: string[], branch: string }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRepliesResponse {
    pub default_action: Option<String>,
    pub secondary_suggestions: Vec<String>,
    pub branch: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scored_candidates: Vec<ScoredSuggestion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<SuggestedRepliesDebugInfo>,
}

impl SuggestedRepliesResponse {
    pub fn is_empty(&self) -> bool {
        self.default_action.is_none() && self.secondary_suggestions.is_empty()
    }

    pub fn top_confidence(&self) -> Option<f32> {
        self.scored_candidates.first().map(|c| c.confidence)
    }
}

/// Pure distribution logic for Suggested Replies:
/// - Sanitizes and filters out empty texts or non-finite confidences.
/// - Clamps confidences to `0.0..=1.0` and deduplicates case-insensitively (keeping highest score).
/// - Sorts candidates by `confidence` descending (`total_cmp`).
/// - If the top candidate has `confidence >= confidence_threshold`, promotes it to
///   `default_action` and places up to `max_secondary_suggestions` (`1..=5`) remaining
///   candidates into `secondary_suggestions`.
/// - Otherwise leaves `default_action = None` and places up to `max_secondary_suggestions`
///   candidates into `secondary_suggestions`.
pub fn distribute_suggestions(
    branch: impl Into<String>,
    candidates: Vec<ScoredSuggestion>,
    confidence_threshold: f32,
    max_secondary_suggestions: usize,
) -> SuggestedRepliesResponse {
    let branch = {
        let b = branch.into();
        let trimmed = b.trim();
        if trimmed.is_empty() {
            "implementation".to_string()
        } else {
            trimmed.to_string()
        }
    };
    let threshold = if confidence_threshold.is_finite() {
        confidence_threshold.clamp(0.0, 1.0)
    } else {
        0.55
    };
    let max_secondary = max_secondary_suggestions.clamp(1, 5);

    let mut deduplicated: Vec<ScoredSuggestion> = Vec::new();
    for item in candidates {
        let text = item.text.trim();
        if text.is_empty() || !item.confidence.is_finite() {
            continue;
        }
        let confidence = item.confidence.clamp(0.0, 1.0);
        if let Some(existing) = deduplicated
            .iter_mut()
            .find(|c| c.text.eq_ignore_ascii_case(text))
        {
            if confidence > existing.confidence {
                existing.confidence = confidence;
                existing.text = text.to_string();
            }
        } else {
            deduplicated.push(ScoredSuggestion {
                text: text.to_string(),
                confidence,
            });
        }
    }

    deduplicated.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));

    if deduplicated.is_empty() {
        return SuggestedRepliesResponse {
            default_action: None,
            secondary_suggestions: Vec::new(),
            branch,
            scored_candidates: Vec::new(),
            debug: None,
        };
    }

    let top = &deduplicated[0];
    let (default_action, secondary_suggestions) = if top.confidence >= threshold {
        let default_action = Some(top.text.clone());
        let secondary = deduplicated
            .iter()
            .skip(1)
            .take(max_secondary)
            .map(|c| c.text.clone())
            .collect();
        (default_action, secondary)
    } else {
        let secondary = deduplicated
            .iter()
            .take(max_secondary)
            .map(|c| c.text.clone())
            .collect();
        (None, secondary)
    };

    SuggestedRepliesResponse {
        default_action,
        secondary_suggestions,
        branch,
        scored_candidates: deduplicated,
        debug: None,
    }
}

/// Fixed Jev-native candidate pool for each macro branch when `use_open_gf = false`.
pub fn fixed_candidates_for_branch(branch: &str) -> Vec<String> {
    match branch.trim().to_ascii_lowercase().as_str() {
        "verification" => vec![
            "Run the targeted test suite and verify all assertions pass".to_string(),
            "Run cargo check / clippy on the modified crates".to_string(),
            "Run a 6-lens subagent self-review (/review 6) on the changes".to_string(),
            "Inspect git diff to verify no unintended edits".to_string(),
            "Commit the verified changes with a clear message".to_string(),
            "Update OpenSpec task checkboxes to reflect completed verification".to_string(),
        ],
        "debugging" => vec![
            "Inspect the failing test output and root-cause the error".to_string(),
            "Add a minimal regression test reproducing the failure".to_string(),
            "Check recent git diff for regressions in the affected module".to_string(),
            "Run the isolated failing test with backtrace enabled".to_string(),
            "Verify the fix across edge cases and rerun the test suite".to_string(),
            "Run a 6-lens subagent review (/review 6) to catch hidden edge cases".to_string(),
        ],
        "review" => vec![
            "Fix the actionable findings from the 6-lens review and rerun tests".to_string(),
            "Run /review 6 to audit correctness, architecture, security, UX, perf, and tests"
                .to_string(),
            "Run the test suite to confirm all review conditions hold".to_string(),
            "Inspect git status and git diff before committing".to_string(),
            "Update documentation or OpenSpec artifacts to match the implementation".to_string(),
            "Commit the reviewed changes".to_string(),
        ],
        "architecture" => vec![
            "Apply the OpenSpec tasks step-by-step (/openspec-apply-change)".to_string(),
            "Run a 6-lens Go/No-Go subagent review on the proposed design".to_string(),
            "Validate OpenSpec artifacts and check task status".to_string(),
            "Inspect module boundaries and public interfaces in the target crates".to_string(),
            "Write unit tests for the core data structures and invariants".to_string(),
            "Prototype the core module and verify it compiles cleanly".to_string(),
        ],
        "refactoring" => vec![
            "Run tests to confirm the refactoring preserves existing behavior".to_string(),
            "Inspect git diff --stat to verify the refactoring scope is clean".to_string(),
            "Run a 6-lens subagent review (/review 6) on the refactored code".to_string(),
            "Remove dead code or unused imports uncovered by the refactor".to_string(),
            "Commit the refactored changes".to_string(),
            "Continue to the next planned task".to_string(),
        ],
        _ => vec![
            "Run unit tests for the modified module and verify everything compiles".to_string(),
            "Run a 6-lens subagent self-review (/review 6) on the new implementation".to_string(),
            "Continue implementing the next pending task in the plan".to_string(),
            "Inspect git diff and commit the completed changes".to_string(),
            "Check edge cases and add targeted unit tests".to_string(),
            "Verify runtime behavior and update OpenSpec task status".to_string(),
        ],
    }
}

/// Deterministic local macro-branch and candidate scorer used when no Jev
/// credential is configured or when the upstream account returns HTTP 402.
pub fn score_locally_from_context(
    transcript_tail: &str,
    use_open_gf_candidates: Option<(String, Vec<String>)>,
    six_lens_findings: Option<&str>,
    confidence_threshold: f32,
    max_secondary_suggestions: usize,
) -> SuggestedRepliesResponse {
    if let Some(findings) = six_lens_findings.map(str::trim).filter(|s| !s.is_empty()) {
        let mut candidates = vec![
            ScoredSuggestion {
                text: format!("Address 6-lens review findings: {findings}"),
                confidence: 0.89,
            },
            ScoredSuggestion {
                text: "Run unit tests to verify the 6-lens review fixes".to_string(),
                confidence: 0.76,
            },
            ScoredSuggestion {
                text: "Re-run /review 6 to confirm all 6 lenses pass with GO".to_string(),
                confidence: 0.68,
            },
            ScoredSuggestion {
                text: "Inspect git diff and commit the verified fixes".to_string(),
                confidence: 0.54,
            },
        ];
        for extra in fixed_candidates_for_branch("review").into_iter().take(2) {
            candidates.push(ScoredSuggestion {
                text: extra,
                confidence: 0.42,
            });
        }
        return distribute_suggestions(
            "review",
            candidates,
            confidence_threshold,
            max_secondary_suggestions,
        );
    }

    let lower = transcript_tail.to_ascii_lowercase();
    let inferred_branch = if lower.contains("no-go")
        || lower.contains("conditional go")
        || lower.contains("6-lens")
        || lower.contains("review")
    {
        "review"
    } else if lower.contains("error[")
        || lower.contains("panicked at")
        || lower.contains("test result: failed")
        || lower.contains("failed")
    {
        "debugging"
    } else if lower.contains("openspec")
        || lower.contains("proposal.md")
        || lower.contains("design.md")
        || lower.contains("architecture")
    {
        "architecture"
    } else if lower.contains("test result: ok")
        || lower.contains("cargo test")
        || lower.contains("compiled")
    {
        "verification"
    } else if lower.contains("refactor") || lower.contains("cleanup") {
        "refactoring"
    } else {
        "implementation"
    };

    let (branch, raw_candidates) = match use_open_gf_candidates {
        Some((b, c)) if !c.is_empty() => (b, c),
        _ => (
            inferred_branch.to_string(),
            fixed_candidates_for_branch(inferred_branch),
        ),
    };

    let scored: Vec<ScoredSuggestion> = raw_candidates
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            // Calibrated decaying confidence so top contextual item clears 0.55 by default.
            let base: f32 = match idx {
                0 => 0.78,
                1 => 0.64,
                2 => 0.51,
                3 => 0.43,
                4 => 0.36,
                _ => 0.28,
            };
            ScoredSuggestion {
                text,
                confidence: base,
            }
        })
        .collect();

    distribute_suggestions(
        branch,
        scored,
        confidence_threshold,
        max_secondary_suggestions,
    )
}

/// Build the SystemOne `choice` question for Iteration 1 (Macro-branching).
pub fn build_macro_branch_choice_question() -> Map<String, Value> {
    let mut criteria = Map::new();
    for &(id, desc) in MACRO_BRANCHES {
        criteria.insert(id.to_string(), Value::String(desc.to_string()));
    }
    let mut questions = Map::new();
    questions.insert(
        "macro_branch".to_string(),
        json!({
            "type": "choice",
            "instructions": "Select the macro conversation branch that best describes the developer's next logical workflow stage based on the recent chat state.",
            "criteria": criteria,
        }),
    );
    questions
}

/// Build batched SystemOne `noul` questions for Iteration 1 when the active Jev
/// provider only supports `noul` (e.g. Jcode subscription gateway).
pub fn build_macro_branch_noul_questions() -> Map<String, Value> {
    let mut questions = Map::new();
    for &(id, desc) in MACRO_BRANCHES {
        questions.insert(
            format!("branch_{id}"),
            json!({
                "type": "noul",
                "instructions": format!("Is '{id}' ({desc}) the most appropriate next macro workflow branch for this conversation?"),
                "criteria": {
                    "true": format!("The conversation is ready for {id}: {desc}"),
                    "false": format!("Another workflow branch is more appropriate than {id}")
                }
            }),
        );
    }
    questions
}

/// Build batched SystemOne `noul` questions for Iteration 2 (Micro-branch candidate scoring).
pub fn build_micro_scoring_noul_questions(
    branch: &str,
    candidates: &[String],
) -> Result<Map<String, Value>> {
    ensure!(
        !candidates.is_empty(),
        "Micro-branch scoring requires at least one candidate"
    );
    let mut questions = Map::new();
    for (idx, candidate) in candidates.iter().take(12).enumerate() {
        questions.insert(
            format!("cand_{idx}"),
            json!({
                "type": "noul",
                "instructions": format!(
                    "In the '{}' workflow branch, is the following action the highest-value immediate next step for the developer? Action: {}",
                    branch, candidate
                ),
                "criteria": {
                    "true": "Directly actionable, high-value immediate next step for the current conversation state",
                    "false": "Premature, redundant, or lower priority than other next steps"
                }
            }),
        );
    }
    Ok(questions)
}

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
            .take(6)
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

async fn complete_open_gf_prompt(system: &str, user_msg: &str) -> Result<(String, String)> {
    let mut errors = Vec::new();

    // 1. Primary Sidecar (OpenAI gpt-5.6-luna / Claude Haiku / registered active provider).
    let sidecar = crate::sidecar::Sidecar::new();
    let sidecar_label = format!("sidecar:{}:{}", sidecar.backend_name(), sidecar.model_name());
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
        if let Ok(provider) = crate::provider::external::instantiate_openrouter_runtime(
            crate::provider::external::OpenRouterRuntimeSpec::NamedProfile {
                name: name.clone(),
                config: profile_cfg,
            },
        ) {
            let label = format!("profile:{name}:{}", provider.model());
            match provider.complete_simple_with_usage(user_msg, system).await {
                Ok((raw, _)) if !raw.trim().is_empty() => return Ok((raw, label)),
                Ok(_) => errors.push(format!("{label} returned empty output")),
                Err(err) => errors.push(format!("{label}: {err}")),
            }
        }
    }

    // 4. Registered external OAuth provider runtimes (Antigravity / Gemini) if available.
    for runtime_key in [
        crate::provider::external::ANTIGRAVITY_RUNTIME,
        crate::provider::external::GEMINI_RUNTIME,
    ] {
        if let Some(provider) = crate::provider::external::instantiate_external_provider(runtime_key)
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

/// Synthesize context-specific next-step candidates via OpenGF (`Sidecar` + fallback routes),
/// returning `(branch, candidates, route_label)`.
pub async fn generate_open_gf_candidates_with_route(
    transcript_tail: &str,
) -> Result<(String, Vec<String>, String)> {
    let system = "You are OpenGF, the generative candidate synthesizer for Jcode's two-stage SystemOne Suggested Replies cascade. Return ONLY valid JSON with the shape: {\"branch\": \"implementation|verification|debugging|review|architecture|refactoring\", \"candidates\": [\"short actionable reply 1\", \"short actionable reply 2\", ...]}. Generate between 3 and 6 concise, concrete next-step user replies (under 90 chars each) tailored to the conversation state. CRITICAL: Write every candidate in the EXACT SAME natural language used by the user in their most recent message (e.g. Russian if the user wrote in Russian, English if in English). Order candidates from the most likely immediate next user reply (#1) to lower-priority alternatives.";
    let user_msg = format!(
        "Recent conversation state:\n{}\n\nProduce the JSON with macro branch and 3..6 concrete next-step user replies in the user's language:",
        crate::util::truncate_str(transcript_tail, 3500)
    );
    let (raw, route_label) = complete_open_gf_prompt(system, &user_msg).await?;
    let (branch, candidates) = parse_open_gf_response(&raw)
        .with_context(|| format!("OpenGF response from {route_label} did not contain valid JSON candidates"))?;
    Ok((branch, candidates, route_label))
}

/// Synthesize context-specific next-step candidates via OpenGF (`Sidecar`).
pub async fn generate_open_gf_candidates(transcript_tail: &str) -> Result<(String, Vec<String>)> {
    let (branch, candidates, _) = generate_open_gf_candidates_with_route(transcript_tail).await?;
    Ok((branch, candidates))
}

/// Calibrate SystemOne `noul` probability with candidate rank prior when the model
/// positively endorses the candidate (`raw_noul >= 0.50`). SystemOne's binary `noul`
/// head evaluates each candidate independently via a sigmoid (typically peaking around
/// `0.55..0.68` on multi-option next-step prompts); blending with the ordered rank prior
/// preserves SystemOne's relative logit re-ranking while mapping positive endorsements
/// onto the `0.55..0.85` Default Action threshold scale. Candidates below `0.50` stay unboosted.
pub fn calibrate_micro_score(raw_noul: f32, idx: usize, from_open_gf: bool) -> f32 {
    let raw = if raw_noul.is_finite() {
        raw_noul.clamp(0.0, 1.0)
    } else {
        0.0
    };
    if raw < 0.50 {
        return raw;
    }
    let rank_prior: f32 = match idx {
        0 => 0.88,
        1 => 0.74,
        2 => 0.64,
        3 => 0.56,
        4 => 0.48,
        _ => 0.42,
    };
    let bonus = if from_open_gf && raw >= 0.55 && idx == 0 {
        0.06
    } else if raw >= 0.58 && idx == 0 {
        0.05
    } else {
        0.0
    };
    (0.60 * raw + 0.40 * rank_prior + bonus).clamp(0.0, 1.0)
}

/// Execute the full two-stage cascade (Macro-Branching -> Micro-Scoring -> Distribution).
/// Gracefully falls back to deterministic local scoring if credentials are unconfigured
/// or if the configured provider returns an error (such as HTTP 402 Insufficient credits).
pub async fn evaluate_cascade(
    transcript_tail: &str,
    config: &crate::config::SuggestedRepliesConfig,
    six_lens_findings: Option<&str>,
) -> SuggestedRepliesResponse {
    let started = std::time::Instant::now();
    let threshold = config.clamped_confidence_threshold();
    let max_secondary = config.clamped_max_secondary();

    if !config.enabled {
        return SuggestedRepliesResponse::default();
    }

    // If 6-lens review findings were just reported, prioritize surfacing them.
    if six_lens_findings.is_some_and(|s| !s.trim().is_empty()) {
        let mut resp = score_locally_from_context(
            transcript_tail,
            None,
            six_lens_findings,
            threshold,
            max_secondary,
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
    let mut open_gf_route = None;
    let open_gf_result = if config.use_open_gf {
        let gf_start = std::time::Instant::now();
        match generate_open_gf_candidates_with_route(transcript_tail).await {
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
                open_gf_route = Some(route);
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

    // Attempt OpenJev / Laya / SystemOne evaluation if a JevClient route is available.
    match crate::jev::JevClient::for_suggestions_with_selector(&config.jev_provider) {
        Ok(client) => {
            let state = json!(crate::util::truncate_str(transcript_tail, 6000));

            // Iteration 1: Determine macro branch (from OpenGF if present, else via SystemOne).
            let branch = if let Some((ref gf_branch, _)) = open_gf_result {
                gf_branch.clone()
            } else if client.supports_choice_questions() {
                let q = build_macro_branch_choice_question();
                match client.evaluate(state.clone(), q).await {
                    Ok(resp) => resp
                        .pointer("/answers/macro_branch/choice")
                        .and_then(Value::as_str)
                        .unwrap_or("implementation")
                        .to_string(),
                    Err(err) => {
                        jev_status = format!("iter1(choice) error: {err}");
                        crate::logging::info(&format!(
                            "[suggested_replies] Iteration 1 (choice) degraded to local scorer: {err}"
                        ));
                        let mut resp = score_locally_from_context(
                            transcript_tail,
                            open_gf_result,
                            six_lens_findings,
                            threshold,
                            max_secondary,
                        );
                        resp.debug = Some(SuggestedRepliesDebugInfo {
                            source: if open_gf_route.is_some() {
                                "opengf+local".to_string()
                            } else {
                                "fixed+local".to_string()
                            },
                            open_gf_status,
                            jev_status,
                            threshold,
                            top_confidence: resp.top_confidence(),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                        return resp;
                    }
                }
            } else {
                let q = build_macro_branch_noul_questions();
                match client.evaluate(state.clone(), q).await {
                    Ok(resp) => MACRO_BRANCHES
                        .iter()
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
                        .unwrap_or_else(|| "implementation".to_string()),
                    Err(err) => {
                        jev_status = format!("iter1(noul) error: {err}");
                        crate::logging::info(&format!(
                            "[suggested_replies] Iteration 1 (noul) degraded to local scorer: {err}"
                        ));
                        let mut resp = score_locally_from_context(
                            transcript_tail,
                            open_gf_result,
                            six_lens_findings,
                            threshold,
                            max_secondary,
                        );
                        resp.debug = Some(SuggestedRepliesDebugInfo {
                            source: if open_gf_route.is_some() {
                                "opengf+local".to_string()
                            } else {
                                "fixed+local".to_string()
                            },
                            open_gf_status,
                            jev_status,
                            threshold,
                            top_confidence: resp.top_confidence(),
                            elapsed_ms: started.elapsed().as_millis() as u64,
                        });
                        return resp;
                    }
                }
            };

            // Iteration 2: Candidate pool & SystemOne logit scoring via `noul`.
            let from_open_gf = open_gf_result
                .as_ref()
                .is_some_and(|(_, cands)| !cands.is_empty());
            let candidate_texts = match open_gf_result.as_ref() {
                Some((_, cands)) if !cands.is_empty() => cands.clone(),
                _ => fixed_candidates_for_branch(&branch),
            };

            if let Ok(micro_q) = build_micro_scoring_noul_questions(&branch, &candidate_texts) {
                let iter2_start = std::time::Instant::now();
                match client.evaluate(state, micro_q).await {
                    Ok(resp) => {
                        let iter2_ms = iter2_start.elapsed().as_millis();
                        let scored: Vec<ScoredSuggestion> = candidate_texts
                            .into_iter()
                            .enumerate()
                            .map(|(idx, text)| {
                                let raw_prob = resp
                                    .pointer(&format!("/answers/cand_{idx}/noul"))
                                    .and_then(Value::as_f64)
                                    .unwrap_or(0.0) as f32;
                                let confidence = calibrate_micro_score(raw_prob, idx, from_open_gf);
                                ScoredSuggestion { text, confidence }
                            })
                            .collect();
                        let mut distributed =
                            distribute_suggestions(branch.clone(), scored, threshold, max_secondary);
                        let top_conf = distributed.top_confidence();
                        jev_status = format!(
                            "ok (selector={}, branch={branch}, iter2={}ms, top={:.2})",
                            config.jev_provider,
                            iter2_ms,
                            top_conf.unwrap_or(0.0)
                        );
                        crate::logging::info(&format!(
                            "[suggested_replies] Cascade complete in {}ms: source={}, branch={branch}, top_conf={:?}, threshold={threshold:.2}, default_action={:?}",
                            started.elapsed().as_millis(),
                            if from_open_gf { "opengf+jev" } else { "fixed+jev" },
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
                        jev_status = format!("iter2(noul) error: {err}");
                        crate::logging::warn(&format!(
                            "[suggested_replies] Iteration 2 (noul) degraded to local scorer: {err}"
                        ));
                    }
                }
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

    let from_open_gf = open_gf_result
        .as_ref()
        .is_some_and(|(_, cands)| !cands.is_empty());
    let mut fallback = score_locally_from_context(
        transcript_tail,
        open_gf_result,
        six_lens_findings,
        threshold,
        max_secondary,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribute_promotes_top_candidate_when_meeting_threshold() {
        let candidates = vec![
            ScoredSuggestion {
                text: "Run cargo test".to_string(),
                confidence: 0.42,
            },
            ScoredSuggestion {
                text: "Fix HTTP 402 fallback".to_string(),
                confidence: 0.81,
            },
            ScoredSuggestion {
                text: "Inspect git diff".to_string(),
                confidence: 0.63,
            },
        ];
        let result = distribute_suggestions("debugging", candidates, 0.55, 5);
        assert_eq!(result.branch, "debugging");
        assert_eq!(result.default_action.as_deref(), Some("Fix HTTP 402 fallback"));
        assert_eq!(
            result.secondary_suggestions,
            vec!["Inspect git diff".to_string(), "Run cargo test".to_string()]
        );

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["defaultAction"], "Fix HTTP 402 fallback");
        assert_eq!(
            json["secondarySuggestions"],
            json!(["Inspect git diff", "Run cargo test"])
        );
        assert_eq!(json["branch"], "debugging");
    }

    #[test]
    fn distribute_leaves_default_action_null_when_below_threshold() {
        let candidates = vec![
            ScoredSuggestion {
                text: "Option A".to_string(),
                confidence: 0.54,
            },
            ScoredSuggestion {
                text: "Option B".to_string(),
                confidence: 0.30,
            },
        ];
        let result = distribute_suggestions("exploration", candidates, 0.55, 5);
        assert_eq!(result.default_action, None);
        assert_eq!(
            result.secondary_suggestions,
            vec!["Option A".to_string(), "Option B".to_string()]
        );

        let json = serde_json::to_value(&result).unwrap();
        assert!(json["defaultAction"].is_null());
    }

    #[test]
    fn distribute_respects_max_secondary_clamping_and_filters_nan() {
        let candidates = (0..8)
            .map(|i| ScoredSuggestion {
                text: format!("Step {i}"),
                confidence: 0.90 - (i as f32) * 0.05,
            })
            .chain([ScoredSuggestion {
                text: "Bad NaN".to_string(),
                confidence: f32::NAN,
            }])
            .collect();

        let result = distribute_suggestions("implementation", candidates, 0.55, 3);
        assert_eq!(result.default_action.as_deref(), Some("Step 0"));
        assert_eq!(
            result.secondary_suggestions,
            vec!["Step 1", "Step 2", "Step 3"]
        );
    }

    #[test]
    fn parse_open_gf_extracts_json_from_fenced_response() {
        let raw = "```json\n{\"branch\": \"Verification\", \"candidates\": [\"cargo test\", \"git status\"]}\n```";
        let (branch, cands) = parse_open_gf_response(raw).unwrap();
        assert_eq!(branch, "verification");
        assert_eq!(cands, vec!["cargo test", "git status"]);
    }

    #[test]
    fn calibrate_micro_score_maps_positive_laya_noul_to_threshold_scale_and_keeps_low_scores_unboosted() {
        // Positive Laya noul (~0.61) on top candidate clears 0.75 threshold after calibration.
        let top = calibrate_micro_score(0.6089, 0, true);
        assert!(top >= 0.75, "expected calibrated top score >= 0.75, got {top}");
        let second = calibrate_micro_score(0.5864, 1, true);
        assert!(top > second);
        // Rejected candidate (< 0.50) is never boosted.
        assert_eq!(calibrate_micro_score(0.42, 0, true), 0.42);
    }
}
