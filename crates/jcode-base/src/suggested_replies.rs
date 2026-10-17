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

/// Synthesize context-specific next-step candidates via OpenGF (`Sidecar`).
pub async fn generate_open_gf_candidates(transcript_tail: &str) -> Result<(String, Vec<String>)> {
    let sidecar = crate::sidecar::Sidecar::new();
    let system = "You are OpenGF, the generative candidate synthesizer for Jcode's two-stage SystemOne Suggested Replies cascade. Return ONLY valid JSON with the shape: {\"branch\": \"implementation|verification|debugging|review|architecture|refactoring\", \"candidates\": [\"short actionable prompt 1\", \"short actionable prompt 2\", ...]}. Generate between 3 and 6 concise, concrete next-step prompts (under 90 chars each) tailored to the conversation state.";
    let user_msg = format!(
        "Recent conversation state:\n{}\n\nProduce the JSON with macro branch and 3..6 concrete next-step prompts:",
        crate::util::truncate_str(transcript_tail, 3500)
    );
    let raw = sidecar.complete(system, &user_msg).await?;
    parse_open_gf_response(&raw).context("OpenGF response did not contain valid JSON candidates")
}

/// Execute the full two-stage cascade (Macro-Branching -> Micro-Scoring -> Distribution).
/// Gracefully falls back to deterministic local scoring if credentials are unconfigured
/// or if the configured provider returns an error (such as HTTP 402 Insufficient credits).
pub async fn evaluate_cascade(
    transcript_tail: &str,
    config: &crate::config::SuggestedRepliesConfig,
    six_lens_findings: Option<&str>,
) -> SuggestedRepliesResponse {
    let threshold = config.clamped_confidence_threshold();
    let max_secondary = config.clamped_max_secondary();

    if !config.enabled {
        return SuggestedRepliesResponse::default();
    }

    // If 6-lens review findings were just reported, prioritize surfacing them.
    if six_lens_findings.is_some_and(|s| !s.trim().is_empty()) {
        return score_locally_from_context(
            transcript_tail,
            None,
            six_lens_findings,
            threshold,
            max_secondary,
        );
    }

    // Stage 1A: If use_open_gf is enabled, synthesize dynamic candidates via Sidecar.
    let open_gf_result = if config.use_open_gf {
        generate_open_gf_candidates(transcript_tail).await.ok()
    } else {
        None
    };

    // Attempt OpenJev / SystemOne evaluation if a JevClient route is available.
    if let Ok(client) = crate::jev::JevClient::for_suggestions() {
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
                    crate::logging::info(&format!(
                        "Suggested replies Iteration 1 (choice) degraded to local scorer: {err}"
                    ));
                    return score_locally_from_context(
                        transcript_tail,
                        open_gf_result,
                        six_lens_findings,
                        threshold,
                        max_secondary,
                    );
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
                    crate::logging::info(&format!(
                        "Suggested replies Iteration 1 (noul) degraded to local scorer: {err}"
                    ));
                    return score_locally_from_context(
                        transcript_tail,
                        open_gf_result,
                        six_lens_findings,
                        threshold,
                        max_secondary,
                    );
                }
            }
        };

        // Iteration 2: Candidate pool & SystemOne logit scoring via `noul`.
        let candidate_texts = match open_gf_result.as_ref() {
            Some((_, cands)) if !cands.is_empty() => cands.clone(),
            _ => fixed_candidates_for_branch(&branch),
        };

        if let Ok(micro_q) = build_micro_scoring_noul_questions(&branch, &candidate_texts)
            && let Ok(resp) = client.evaluate(state, micro_q).await
        {
            let scored: Vec<ScoredSuggestion> = candidate_texts
                .into_iter()
                .enumerate()
                .map(|(idx, text)| {
                    let prob = resp
                        .pointer(&format!("/answers/cand_{idx}/noul"))
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0) as f32;
                    ScoredSuggestion {
                        text,
                        confidence: prob,
                    }
                })
                .collect();
            return distribute_suggestions(branch, scored, threshold, max_secondary);
        }
    }

    score_locally_from_context(
        transcript_tail,
        open_gf_result,
        six_lens_findings,
        threshold,
        max_secondary,
    )
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
}
