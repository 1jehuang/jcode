use super::{
    MACRO_BRANCHES, MAX_MICRO_CANDIDATES, ScoredSuggestion, SuggestedRepliesContext,
    SuggestedRepliesResponse, classify_suggestion_for_goal, distribute_suggestions,
    push_unique_case_insensitive,
};
use anyhow::{Result, ensure};
use serde_json::{Map, Value, json};

/// Fixed English Jev-native candidate pool for each macro branch.
pub fn fixed_candidates_for_branch(branch: &str) -> Vec<String> {
    localized_branch_candidates(branch, false)
}

fn localized_branch_candidates(branch: &str, cyrillic: bool) -> Vec<String> {
    let key = branch.trim().to_ascii_lowercase();
    if cyrillic {
        match key.as_str() {
            "goal_delivery" => vec![
                "Закоммитить проверенные изменения и завершить цель".to_string(),
                "Прогнать финальные тесты перед отправкой изменений в PR".to_string(),
                "Проверить git status и git diff перед финализацией".to_string(),
                "Обновить чекбоксы задач в OpenSpec и закрыть план".to_string(),
                "Выполнить 6-линзовое ревью (/review 6) перед сдачей результата".to_string(),
                "Пересобрать и перезагрузить бинарник через selfdev build-reload".to_string(),
            ],
            "planning" => vec![
                "Декомпозировать цель на конкретные шаги в списке задач (todo)".to_string(),
                "Приступить к выполнению следующей задачи из плана (/opsx:apply)".to_string(),
                "Изучить границы модулей и публичные интерфейсы целевых крейтов".to_string(),
                "Проверить валидность артефактов OpenSpec (openspec validate --all --strict)"
                    .to_string(),
                "Написать модульные тесты для ключевых инвариантов цели".to_string(),
                "Закоммитить согласованный план и приступить к реализации".to_string(),
            ],
            "verification" => vec![
                "Запустить целевые тесты и убедиться, что все проверки проходят".to_string(),
                "Выполнить cargo check и clippy для изменённых крейтов".to_string(),
                "Запустить 6-линзовое само-ревью (/review 6) по внесённым изменениям".to_string(),
                "Проверить git diff на отсутствие лишних правок".to_string(),
                "Закоммитить проверенные изменения с понятным сообщением".to_string(),
                "Обновить чекбоксы задач в OpenSpec после успешной проверки".to_string(),
            ],
            "debugging" => vec![
                "Проанализировать вывод падающего теста и найти корневую причину ошибки"
                    .to_string(),
                "Добавить минимальный регрессионный тест, воспроизводящий сбой".to_string(),
                "Проверить недавний git diff на регрессии в затронутом модуле".to_string(),
                "Запустить проблемный тест изолированно с подробным логом".to_string(),
                "Проверить исправление на граничных случаях и перезапустить тесты".to_string(),
                "Выполнить 6-линзовое ревью (/review 6) для поиска скрытых проблем".to_string(),
            ],
            "review" => vec![
                "Исправить замечания из 6-линзового ревью и перезапустить тесты".to_string(),
                "Запустить /review 6 для аудита корректности, архитектуры, безопасности и тестов"
                    .to_string(),
                "Прогнать тестовый набор для подтверждения всех условий ревью".to_string(),
                "Проверить git status и git diff перед коммитом".to_string(),
                "Обновить документацию и артефакты OpenSpec в соответствии с кодом".to_string(),
                "Закоммитить проверенные изменения".to_string(),
            ],
            "architecture" => vec![
                "Приступить к выполнению задач по плану (/opsx:apply)".to_string(),
                "Провести 6-линзовое Go/No-Go ревью предложенной архитектуры".to_string(),
                "Проверить валидность артефактов OpenSpec (openspec validate --all --strict)"
                    .to_string(),
                "Изучить границы модулей и публичные интерфейсы целевых крейтов".to_string(),
                "Написать модульные тесты для ключевых структур данных и инвариантов".to_string(),
                "Реализовать прототип основного модуля и проверить сборку".to_string(),
            ],
            "refactoring" => vec![
                "Запустить тесты и убедиться, что рефакторинг сохраняет поведение".to_string(),
                "Проверить git diff --stat, чтобы убедиться в чистоте изменений".to_string(),
                "Запустить 6-линзовое ревью (/review 6) по отрефакторенному коду".to_string(),
                "Удалить неиспользуемый код и лишние импорты".to_string(),
                "Закоммитить результаты рефакторинга".to_string(),
                "Перейти к следующей запланированной задаче".to_string(),
            ],
            _ => vec![
                "Запустить модульные тесты для изменённого модуля и проверить сборку".to_string(),
                "Запустить 6-линзовое ревью (/review 6) для новой реализации".to_string(),
                "Продолжить реализацию следующей задачи из плана".to_string(),
                "Проверить git diff и закоммитить готовые изменения".to_string(),
                "Проверить граничные случаи и добавить модульные тесты".to_string(),
                "Пересобрать и перезагрузить бинарник через selfdev build-reload".to_string(),
            ],
        }
    } else {
        match key.as_str() {
            "goal_delivery" => vec![
                "Commit the verified changes and deliver the final goal".to_string(),
                "Run the final test suite before pushing to the PR".to_string(),
                "Inspect git status and git diff before finalizing".to_string(),
                "Update OpenSpec task checkboxes to mark the goal complete".to_string(),
                "Run a 6-lens subagent self-review (/review 6) before delivery".to_string(),
                "Rebuild and reload the binary via selfdev build-reload".to_string(),
            ],
            "planning" => vec![
                "Decompose the goal into concrete actionable steps in the todo list".to_string(),
                "Continue implementing the next pending task in the plan".to_string(),
                "Inspect module boundaries and public interfaces in the target crates".to_string(),
                "Validate OpenSpec artifacts and check task status".to_string(),
                "Write unit tests for the core data structures and invariants".to_string(),
                "Commit the agreed plan and start implementation".to_string(),
            ],
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
                "Update documentation or OpenSpec artifacts to match the implementation"
                    .to_string(),
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
                "Run a 6-lens subagent self-review (/review 6) on the new implementation"
                    .to_string(),
                "Continue implementing the next pending task in the plan".to_string(),
                "Inspect git diff and commit the completed changes".to_string(),
                "Check edge cases and add targeted unit tests".to_string(),
                "Verify runtime behavior and update OpenSpec task status".to_string(),
            ],
        }
    }
}

/// Build a dynamic, language-matched candidate pool from `SuggestedRepliesContext`:
/// 1. Extracted assistant options / explicit questions / slash commands.
/// 2. Active pending `todo` items framed in the user's language.
/// 3. Localized primary branch candidates.
/// 4. Localized cross-branch workflow candidates so up to `MAX_MICRO_CANDIDATES` (`21`)
///    unique candidates are available when `max_secondary_suggestions` is up to `20`.
pub fn contextual_candidates_for_branch(
    branch: &str,
    ctx: &SuggestedRepliesContext,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();

    for opt in &ctx.assistant_options {
        push_unique_case_insensitive(&mut out, opt.clone());
    }

    for todo in ctx.pending_todos.iter().take(2) {
        let cleaned = todo.trim();
        if !cleaned.is_empty() {
            let prompt = if ctx.language_is_cyrillic {
                format!("Выполнить следующую задачу: {cleaned}")
            } else {
                format!("Work on next todo: {cleaned}")
            };
            push_unique_case_insensitive(&mut out, prompt);
        }
    }

    for item in localized_branch_candidates(branch, ctx.language_is_cyrillic) {
        push_unique_case_insensitive(&mut out, item);
    }

    // Top up from the remaining macro branches so that configurations up to
    // `max_secondary_suggestions = 20` (requiring 21 total candidates) have a full pool.
    for &(other_branch, _) in MACRO_BRANCHES {
        if out.len() >= MAX_MICRO_CANDIDATES {
            break;
        }
        if !other_branch.eq_ignore_ascii_case(branch.trim()) {
            for item in localized_branch_candidates(other_branch, ctx.language_is_cyrillic) {
                push_unique_case_insensitive(&mut out, item);
                if out.len() >= MAX_MICRO_CANDIDATES {
                    break;
                }
            }
        }
    }

    out.truncate(MAX_MICRO_CANDIDATES);
    out
}

/// Merge user-configured constant `custom_prompts` with dynamic `base_candidates`,
/// deduplicating case-insensitively and capping at `max_candidates` (`<= MAX_MICRO_CANDIDATES`).
/// User `custom_prompts` are inserted ahead of generic overflow candidates so they are
/// guaranteed to be evaluated by SystemOne (`Laya`/`OpenJev`) or the local scorer.
pub fn merge_custom_prompts(
    base_candidates: Vec<String>,
    custom_prompts: &[String],
    max_candidates: usize,
) -> Vec<String> {
    let cap = max_candidates.clamp(1, MAX_MICRO_CANDIDATES);
    let sanitized_custom: Vec<String> = custom_prompts
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if sanitized_custom.is_empty() {
        return base_candidates.into_iter().take(cap).collect();
    }

    let mut out: Vec<String> = Vec::new();
    // Keep top primary dynamic candidates first (up to cap - custom_count, at least 2),
    // then insert all user custom_prompts, then fill any remaining slots from base_candidates.
    let reserve_for_custom = sanitized_custom.len().min(cap);
    let primary_dynamic_slots = cap.saturating_sub(reserve_for_custom).max(1.min(cap));

    for item in base_candidates.iter().take(primary_dynamic_slots) {
        if out.len() >= cap {
            break;
        }
        push_unique_case_insensitive(&mut out, item.clone());
    }
    for custom in &sanitized_custom {
        if out.len() >= cap {
            break;
        }
        push_unique_case_insensitive(&mut out, custom.clone());
    }
    for item in base_candidates {
        if out.len() >= cap {
            break;
        }
        push_unique_case_insensitive(&mut out, item);
    }

    out
}

pub(super) fn infer_macro_branch_from_text(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("no-go")
        || lower.contains("conditional go")
        || lower.contains("6-lens")
        || lower.contains("review")
        || lower.contains("ревью")
    {
        "review"
    } else if lower.contains("error[")
        || lower.contains("panicked at")
        || lower.contains("test result: failed")
        || lower.contains("failed")
        || lower.contains("ошибк")
        || lower.contains("упал")
    {
        "debugging"
    } else if lower.contains("openspec")
        || lower.contains("proposal.md")
        || lower.contains("design.md")
        || lower.contains("architecture")
        || lower.contains("спецификац")
        || lower.contains("архитектур")
    {
        "architecture"
    } else if lower.contains("test result: ok")
        || lower.contains("cargo test")
        || lower.contains("compiled")
        || lower.contains("тест")
    {
        "verification"
    } else if lower.contains("refactor") || lower.contains("cleanup") || lower.contains("рефактор")
    {
        "refactoring"
    } else {
        "implementation"
    }
}

/// Deterministic local macro-branch and candidate scorer using structured `SuggestedRepliesContext`
/// and optional user `custom_prompts`.
pub fn score_locally_with_context(
    ctx: &SuggestedRepliesContext,
    use_open_gf_candidates: Option<(String, Vec<String>)>,
    custom_prompts: &[String],
    six_lens_findings: Option<&str>,
    confidence_threshold: f32,
    max_secondary_suggestions: usize,
) -> SuggestedRepliesResponse {
    let max_secondary = max_secondary_suggestions.clamp(1, 20);
    let desired_pool = (max_secondary + 1).clamp(6, MAX_MICRO_CANDIDATES);

    if let Some(raw_findings) = six_lens_findings.map(str::trim).filter(|s| !s.is_empty()) {
        let extracted = super::extract_six_lens_review_findings(raw_findings);
        let findings = extracted.as_deref().unwrap_or(raw_findings);
        let mut raw_list = if ctx.language_is_cyrillic {
            vec![
                format!("Исправить замечания 6-линзового ревью: {findings}"),
                "Запустить модульные тесты для проверки исправлений по ревью".to_string(),
                "Повторно запустить /review 6 и убедиться в статусе GO".to_string(),
                "Проверить git diff и закоммитить исправления".to_string(),
            ]
        } else {
            vec![
                format!("Address 6-lens review findings: {findings}"),
                "Run unit tests to verify the 6-lens review fixes".to_string(),
                "Re-run /review 6 to confirm all 6 lenses pass with GO".to_string(),
                "Inspect git diff and commit the verified fixes".to_string(),
            ]
        };
        for extra in contextual_candidates_for_branch("review", ctx) {
            push_unique_case_insensitive(&mut raw_list, extra);
        }
        let merged = merge_custom_prompts(raw_list, custom_prompts, desired_pool);
        let candidates: Vec<ScoredSuggestion> = merged
            .into_iter()
            .enumerate()
            .map(|(idx, text)| {
                let confidence = match idx {
                    0 => 0.89,
                    1 => 0.76,
                    2 => 0.68,
                    3 => 0.54,
                    _ => (0.48 - (idx.saturating_sub(4) as f32) * 0.015).max(0.18),
                };
                ScoredSuggestion { text, confidence }
            })
            .collect();
        let mut resp =
            distribute_suggestions("review", candidates, confidence_threshold, max_secondary);
        resp.populate_goal_categories(custom_prompts);
        return resp;
    }

    let inferred_branch = infer_macro_branch_from_text(&ctx.transcript_tail);
    let contextual_pool = contextual_candidates_for_branch(inferred_branch, ctx);

    let (branch, base_candidates) = match use_open_gf_candidates {
        Some((b, mut c)) if !c.is_empty() => {
            for extra in contextual_pool {
                if c.len() >= desired_pool {
                    break;
                }
                push_unique_case_insensitive(&mut c, extra);
            }
            (b, c)
        }
        _ => (inferred_branch.to_string(), contextual_pool),
    };

    let merged = merge_custom_prompts(base_candidates, custom_prompts, desired_pool);

    let scored: Vec<ScoredSuggestion> = merged
        .into_iter()
        .enumerate()
        .map(|(idx, text)| {
            let base: f32 = match idx {
                0 => 0.78,
                1 => 0.64,
                2 => 0.51,
                3 => 0.43,
                4 => 0.36,
                _ => (0.32 - (idx.saturating_sub(5) as f32) * 0.01).max(0.15),
            };
            ScoredSuggestion {
                text,
                confidence: base,
            }
        })
        .collect();

    let mut resp = distribute_suggestions(branch, scored, confidence_threshold, max_secondary);
    resp.populate_goal_categories(custom_prompts);
    resp
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
    let ctx = SuggestedRepliesContext::from_transcript(transcript_tail);
    score_locally_with_context(
        &ctx,
        use_open_gf_candidates,
        &[],
        six_lens_findings,
        confidence_threshold,
        max_secondary_suggestions,
    )
}

/// Build the structured JSON `state` passed to SystemOne (`/v1/systemone`) in both
/// Iteration 1 (`candidates` empty) and Iteration 2 (`candidates` populated as `cand_0..cand_N`).
pub fn build_structured_systemone_state(
    branch: Option<&str>,
    ctx: &SuggestedRepliesContext,
    candidates: &[String],
) -> Value {
    let mut state = Map::new();
    if let Some(b) = branch.map(str::trim).filter(|s| !s.is_empty()) {
        state.insert("workflow_branch".to_string(), Value::String(b.to_string()));
    }
    state.insert(
        "language".to_string(),
        Value::String(if ctx.language_is_cyrillic { "ru" } else { "en" }.to_string()),
    );
    if let Some(ref user_msg) = ctx.last_user_message {
        state.insert(
            "last_user_message".to_string(),
            Value::String(crate::util::truncate_str(user_msg, 1200).to_string()),
        );
    }
    if let Some(ref assistant_msg) = ctx.last_assistant_message {
        state.insert(
            "last_assistant_message".to_string(),
            Value::String(crate::util::truncate_str(assistant_msg, 1800).to_string()),
        );
    }
    if !ctx.assistant_options.is_empty() {
        state.insert(
            "assistant_options".to_string(),
            json!(ctx.assistant_options),
        );
    }
    if !ctx.pending_todos.is_empty() {
        state.insert("pending_todos".to_string(), json!(ctx.pending_todos));
    }
    state.insert(
        "recent_transcript".to_string(),
        Value::String(crate::util::truncate_str(&ctx.transcript_tail, 3200).to_string()),
    );

    if !candidates.is_empty() {
        let branch_ref = branch.unwrap_or("implementation");
        let mut cand_map = Map::new();
        let mut goal_labels = Map::new();
        for (idx, text) in candidates.iter().take(MAX_MICRO_CANDIDATES).enumerate() {
            let key = format!("cand_{idx}");
            cand_map.insert(key.clone(), Value::String(text.clone()));
            let cat = classify_suggestion_for_goal(text, branch_ref, &[]);
            goal_labels.insert(key, Value::String(cat.to_string()));
        }
        state.insert("candidates".to_string(), Value::Object(cand_map));
        state.insert(
            "candidate_goal_labels".to_string(),
            Value::Object(goal_labels),
        );
    }

    Value::Object(state)
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
            "instructions": "Select the macro conversation branch that best describes the developer's next logical workflow stage based on the structured chat state.",
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

/// Build Iteration 2 SystemOne micro-scoring questions over `state.candidates.cand_0..cand_N`.
/// When `include_choice && candidates.len() >= 2`, includes a comparative `"best_candidate"`
/// (`type: "choice"`) alongside `"cand_0..cand_N"` (`type: "noul"`) in a single batch.
pub fn build_micro_scoring_questions(
    branch: &str,
    candidates: &[String],
    include_choice: bool,
) -> Result<Map<String, Value>> {
    ensure!(
        !candidates.is_empty(),
        "Micro-branch scoring requires at least one candidate"
    );
    let capped: Vec<&String> = candidates.iter().take(MAX_MICRO_CANDIDATES).collect();
    let mut questions = Map::new();

    if include_choice && capped.len() >= 2 {
        let mut choice_criteria = Map::new();
        for (idx, candidate) in capped.iter().enumerate() {
            let cat = classify_suggestion_for_goal(candidate, branch, &[]);
            choice_criteria.insert(
                format!("cand_{idx}"),
                Value::String(format!(
                    "Candidate cand_{idx} [goal_label={cat}] in state.candidates: {}",
                    crate::util::truncate_str(candidate, 140)
                )),
            );
        }
        questions.insert(
            "best_candidate".to_string(),
            json!({
                "type": "choice",
                "instructions": format!(
                    "In the '{}' workflow branch, select which candidate reply in state.candidates is the single highest-value immediate next step toward achieving the user's goal.",
                    branch
                ),
                "criteria": choice_criteria,
            }),
        );
    }

    for (idx, candidate) in capped.iter().enumerate() {
        let cat = classify_suggestion_for_goal(candidate, branch, &[]);
        questions.insert(
            format!("cand_{idx}"),
            json!({
                "type": "noul",
                "instructions": format!(
                    "Evaluate candidate cand_{idx} [goal_label={cat}] in state.candidates for the '{}' workflow branch: is it a directly actionable, high-value immediate next reply toward reaching the user's goal? ({})",
                    branch,
                    crate::util::truncate_str(candidate, 140)
                ),
                "criteria": {
                    "true": "Directly actionable, context-matching, high-value immediate next reply that advances the user toward reaching their goal",
                    "false": "Premature, redundant, wrong language, or lower priority than other next steps toward the goal"
                }
            }),
        );
    }
    Ok(questions)
}

/// Build batched SystemOne `noul` questions for Iteration 2 (backward-compatible wrapper).
pub fn build_micro_scoring_noul_questions(
    branch: &str,
    candidates: &[String],
) -> Result<Map<String, Value>> {
    build_micro_scoring_questions(branch, candidates, false)
}
