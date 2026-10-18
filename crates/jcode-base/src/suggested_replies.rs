//! Two-stage cascading Suggested Replies with Default Action Focus.
//!
//! Combines OpenJev / Laya SystemOne (`/v1/systemone`, calibrated logit decisions
//! via joint `choice` and `noul` over a structured JSON `state`) with OpenGF
//! (`Sidecar` / named profile generative candidate synthesis with `"store": false`),
//! assistant question/option extraction, user-defined constant questions (`custom_prompts`),
//! and a language-aware deterministic local fallback.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::sync::OnceLock;

/// Maximum number of candidates evaluated in a single Iteration 2 micro-scoring batch.
/// Allows 1 Default Action + up to 20 secondary suggestions (`21` candidates total),
/// keeping `1 (best_candidate choice) + 21 (cand_0..cand_20 noul) = 22 <= MAX_QUESTIONS (24)`.
pub const MAX_MICRO_CANDIDATES: usize = 21;

/// Macro conversation branches for Iteration 1 classification toward reaching the user's goal.
pub const MACRO_BRANCHES: &[(&str, &str)] = &[
    (
        "goal_delivery",
        "Completing, shipping, committing, merging, or delivering the user's final goal and outcome",
    ),
    (
        "planning",
        "Decomposing the user's goal into actionable steps, todos, or milestones",
    ),
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

/// Canonical goal-reaching classification categories (`category`) assigned to each suggested reply:
/// - `"goal"`: Directly achieves/delivers the user's goal (commit, push, PR, final action).
/// - `"plan"`: Advances the next todo, plan, or OpenSpec step toward the goal.
/// - `"impl"`: Implements code, features, or builds/reloads toward the goal.
/// - `"verify"`: Runs tests, compiler checks, or validation to close the feedback loop.
/// - `"debug"`: Investigates or fixes errors, regressions, or blockers.
/// - `"review"`: Audits code quality/security (6-lens review, git diff) before delivery.
/// - `"refactor"`: Simplifies or cleans up code while preserving behavior.
/// - `"custom"`: User-configured constant question (`custom_prompts`).
pub const GOAL_CLASSIFICATION_CATEGORIES: &[(&str, &str, &str)] = &[
    ("goal", "[цель]", "[goal]"),
    ("plan", "[план]", "[plan]"),
    ("impl", "[код]", "[impl]"),
    ("verify", "[тест]", "[verify]"),
    ("debug", "[баг]", "[debug]"),
    ("review", "[ревью]", "[review]"),
    ("refactor", "[рефактор]", "[refactor]"),
    ("custom", "[свой]", "[custom]"),
];

/// Classify how a candidate suggestion advances the user toward reaching their goal.
pub fn classify_suggestion_for_goal(
    text: &str,
    branch: &str,
    custom_prompts: &[String],
) -> &'static str {
    let trimmed = text.trim();
    if !trimmed.is_empty()
        && custom_prompts
            .iter()
            .any(|c| c.trim().eq_ignore_ascii_case(trimmed))
    {
        return "custom";
    }
    let lower = trimmed.to_lowercase();

    if lower.contains("/review")
        || lower.contains("6-линз")
        || lower.contains("6-lens")
        || lower.contains("ревью")
        || lower.contains("review")
        || lower.contains("git diff")
        || lower.contains("git status")
        || lower.contains("аудит")
    {
        return "review";
    }
    if lower.contains("падающ")
        || lower.contains("ошибк")
        || lower.contains("сбой")
        || lower.contains("регресс")
        || lower.contains("root-cause")
        || lower.contains("failing")
        || lower.contains("backtrace")
        || lower.contains("отлад")
        || lower.contains("debug")
    {
        return "debug";
    }
    if lower.contains("рефактор")
        || lower.contains("refactor")
        || lower.contains("неиспользуем")
        || lower.contains("dead code")
        || lower.contains("unused import")
        || lower.contains("лишние импорт")
    {
        return "refactor";
    }
    if lower.contains("следующую задачу")
        || lower.contains("следующей запланированной")
        || lower.contains("next todo")
        || lower.contains("next pending task")
        || lower.contains("next planned task")
        || lower.contains("/opsx:")
        || lower.contains("/openspec")
        || lower.contains("openspec")
        || lower.contains("чекбокс")
        || lower.contains("checkbox")
        || lower.contains("по плану")
        || lower.contains("декомпоз")
    {
        return "plan";
    }
    if lower.contains("закоммит")
        || lower.contains("коммит")
        || lower.contains("commit")
        || lower.contains("push")
        || lower.contains("merge")
        || lower.contains("влить")
        || lower.contains("завершить цель")
        || lower.contains("финализир")
        || lower.contains("deliver")
        || lower.contains("ship")
    {
        return "goal";
    }
    if lower.contains("тест")
        || lower.contains("test")
        || lower.contains("cargo check")
        || lower.contains("clippy")
        || lower.contains("assertions pass")
        || lower.contains("проверки проходят")
        || lower.contains("проверить сборку")
        || lower.contains("verify runtime")
        || lower.contains("compiles")
    {
        return "verify";
    }
    if lower.contains("реализ")
        || lower.contains("implement")
        || lower.contains("selfdev")
        || lower.contains("build-reload")
        || lower.contains("прототип")
        || lower.contains("prototype")
        || lower.contains("границы модулей")
        || lower.contains("module boundaries")
    {
        return "impl";
    }

    match branch.trim().to_ascii_lowercase().as_str() {
        "goal" | "goal_delivery" | "delivery" => "goal",
        "plan" | "planning" | "architecture" => "plan",
        "verify" | "verification" => "verify",
        "debug" | "debugging" => "debug",
        "review" => "review",
        "refactor" | "refactoring" => "refactor",
        "custom" => "custom",
        _ => "impl",
    }
}

/// Format a canonical goal classification category (`"goal"`, `"plan"`, `"impl"`, `"verify"`,
/// `"debug"`, `"review"`, `"refactor"`, `"custom"`) as a localized compact UI badge.
pub fn format_goal_category_badge(category: &str, cyrillic: bool) -> &'static str {
    let key = category.trim().to_ascii_lowercase();
    let idx = match key.as_str() {
        "goal" | "goal_delivery" | "delivery" => 0,
        "plan" | "planning" | "architecture" => 1,
        "impl" | "implementation" => 2,
        "verify" | "verification" => 3,
        "debug" | "debugging" => 4,
        "review" => 5,
        "refactor" | "refactoring" => 6,
        "custom" => 7,
        _ => 2,
    };
    if cyrillic {
        GOAL_CLASSIFICATION_CATEGORIES[idx].1
    } else {
        GOAL_CLASSIFICATION_CATEGORIES[idx].2
    }
}

/// Structured conversation & workflow context passed to OpenGF and SystemOne.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedRepliesContext {
    pub transcript_tail: String,
    pub last_user_message: Option<String>,
    pub last_assistant_message: Option<String>,
    pub assistant_options: Vec<String>,
    pub pending_todos: Vec<String>,
    pub language_is_cyrillic: bool,
}

impl SuggestedRepliesContext {
    /// Parse a recent transcript string (formatted with `[user]: ...` / `[assistant]: ...`
    /// blocks or raw text) into a structured `SuggestedRepliesContext`.
    pub fn from_transcript(transcript_tail: &str) -> Self {
        let mut last_user_message: Option<String> = None;
        let mut last_assistant_message: Option<String> = None;

        let mut current_role: Option<&str> = None;
        let mut current_buf = String::new();
        let flush = |role: Option<&str>,
                     buf: &mut String,
                     last_user: &mut Option<String>,
                     last_assistant: &mut Option<String>| {
            let trimmed = buf.trim();
            if !trimmed.is_empty() {
                match role {
                    Some("user") => *last_user = Some(trimmed.to_string()),
                    Some("assistant") => *last_assistant = Some(trimmed.to_string()),
                    _ => {}
                }
            }
            buf.clear();
        };

        for line in transcript_tail.lines() {
            if let Some(rest) = line.strip_prefix("[user]:") {
                flush(
                    current_role,
                    &mut current_buf,
                    &mut last_user_message,
                    &mut last_assistant_message,
                );
                current_role = Some("user");
                current_buf.push_str(rest.trim_start());
                current_buf.push('\n');
            } else if let Some(rest) = line.strip_prefix("[assistant]:") {
                flush(
                    current_role,
                    &mut current_buf,
                    &mut last_user_message,
                    &mut last_assistant_message,
                );
                current_role = Some("assistant");
                current_buf.push_str(rest.trim_start());
                current_buf.push('\n');
            } else if let Some(rest) = line.strip_prefix("[swarm]:") {
                flush(
                    current_role,
                    &mut current_buf,
                    &mut last_user_message,
                    &mut last_assistant_message,
                );
                current_role = Some("swarm");
                current_buf.push_str(rest.trim_start());
                current_buf.push('\n');
            } else {
                current_buf.push_str(line);
                current_buf.push('\n');
            }
        }
        flush(
            current_role,
            &mut current_buf,
            &mut last_user_message,
            &mut last_assistant_message,
        );

        let assistant_source = last_assistant_message
            .as_deref()
            .unwrap_or(transcript_tail);
        let assistant_options = extract_assistant_options(assistant_source);

        let language_probe = last_user_message
            .as_deref()
            .or(last_assistant_message.as_deref())
            .unwrap_or(transcript_tail);
        let language_is_cyrillic = detect_cyrillic_language(language_probe);

        Self {
            transcript_tail: transcript_tail.to_string(),
            last_user_message,
            last_assistant_message,
            assistant_options,
            pending_todos: Vec::new(),
            language_is_cyrillic,
        }
    }

    /// Attach active session `todo` items (`pending` / `in_progress`).
    pub fn with_pending_todos(mut self, pending_todos: Vec<String>) -> Self {
        self.pending_todos = pending_todos
            .into_iter()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .take(8)
            .collect();
        self
    }
}

/// Detect whether `text` is primarily written in Cyrillic (e.g. Russian).
pub fn detect_cyrillic_language(text: &str) -> bool {
    let mut cyrillic = 0usize;
    let mut latin = 0usize;
    for ch in text.chars() {
        if ('\u{0400}'..='\u{04FF}').contains(&ch) {
            cyrillic += 1;
        } else if ch.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    cyrillic >= 3 && (cyrillic * 2 >= latin || cyrillic >= 8)
}

/// Extract actionable options, numbered/bulleted items, or slash commands from the
/// closing lines of `last_assistant_message`.
pub fn extract_assistant_options(last_assistant_message: &str) -> Vec<String> {
    let trimmed = last_assistant_message.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let mut options: Vec<String> = Vec::new();
    let mut in_code_block = false;
    let tail_lines: Vec<&str> = trimmed.lines().rev().take(28).collect();

    for raw_line in tail_lines.into_iter().rev() {
        let line = raw_line.trim();
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block || line.is_empty() {
            continue;
        }

        // 1. Check for numbered list items: "1. Foo" or "1) Foo"
        let numbered = line
            .strip_prefix(|c: char| c.is_ascii_digit())
            .and_then(|rest| {
                let rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
                rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))
            });
        if let Some(item) = numbered {
            let cleaned = clean_extracted_option(item);
            if is_viable_option_candidate(&cleaned) {
                push_unique_case_insensitive(&mut options, cleaned);
                continue;
            }
        }

        // 2. Check for bullet items ("- ...", "* ...", "• ...") that look like actionable choices
        let bulleted = line
            .strip_prefix("- ")
            .or_else(|| line.strip_prefix("* "))
            .or_else(|| line.strip_prefix("• "));
        if let Some(item) = bulleted {
            let cleaned = clean_extracted_option(item);
            if is_viable_option_candidate(&cleaned) && cleaned.chars().count() <= 96 {
                push_unique_case_insensitive(&mut options, cleaned);
                continue;
            }
        }

        // 3. Extract explicit backticked slash commands (e.g. `/opsx:apply`, `/review 6`, `/suggestions debug`)
        let mut rest = line;
        while let Some(start) = rest.find('`') {
            let after_start = &rest[start + 1..];
            if let Some(end) = after_start.find('`') {
                let code_span = after_start[..end].trim();
                if code_span.starts_with('/')
                    && code_span.len() >= 3
                    && code_span.len() <= 48
                    && !code_span.contains("//")
                    && !code_span[1..].contains('/')
                {
                    push_unique_case_insensitive(&mut options, code_span.to_string());
                }
                rest = &after_start[end + 1..];
            } else {
                break;
            }
        }
    }

    options.into_iter().take(6).collect()
}

fn clean_extracted_option(raw: &str) -> String {
    let without_bold = raw.replace("**", "");
    let trimmed = without_bold
        .trim()
        .trim_matches(|c: char| c == '`' || c == '"' || c == '«' || c == '»')
        .trim_end_matches([';', ',']);
    let head = if let Some((left, _)) = trimmed.split_once(" — ") {
        left.trim()
    } else if let Some((left, right)) = trimmed.split_once(": ") {
        if left.chars().count() >= 10 && left.chars().count() <= 80 {
            left.trim()
        } else {
            right.trim()
        }
    } else {
        trimmed
    };
    head.chars().take(110).collect::<String>().trim().to_string()
}

fn is_viable_option_candidate(text: &str) -> bool {
    let char_count = text.chars().count();
    (4..=110).contains(&char_count)
        && !text.starts_with("http://")
        && !text.starts_with("https://")
        && !text.starts_with('[')
}

fn push_unique_case_insensitive(list: &mut Vec<String>, candidate: String) {
    let trimmed = candidate.trim();
    if trimmed.is_empty() {
        return;
    }
    if !list
        .iter()
        .any(|existing| existing.to_lowercase() == trimmed.to_lowercase())
    {
        list.push(trimmed.to_string());
    }
}

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
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub candidate_categories: std::collections::HashMap<String, String>,
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

    /// Look up the calibrated model confidence (`0.0..=1.0`) for a suggestion text.
    pub fn confidence_for_text(&self, text: &str) -> Option<f32> {
        let target = text.trim();
        if target.is_empty() {
            return None;
        }
        self.scored_candidates
            .iter()
            .find(|c| c.text.trim() == target)
            .or_else(|| {
                let lower = target.to_lowercase();
                self.scored_candidates
                    .iter()
                    .find(|c| c.text.trim().to_lowercase() == lower)
            })
            .map(|c| c.confidence)
    }

    /// Populate `candidate_categories` for all candidates, tagging any match in `custom_prompts` as `"custom"`.
    pub fn populate_goal_categories(&mut self, custom_prompts: &[String]) {
        for cand in &self.scored_candidates {
            let cat = classify_suggestion_for_goal(&cand.text, &self.branch, custom_prompts);
            self.candidate_categories
                .insert(cand.text.trim().to_lowercase(), cat.to_string());
        }
        if let Some(ref def) = self.default_action {
            let cat = classify_suggestion_for_goal(def, &self.branch, custom_prompts);
            self.candidate_categories
                .insert(def.trim().to_lowercase(), cat.to_string());
        }
        for sec in &self.secondary_suggestions {
            let cat = classify_suggestion_for_goal(sec, &self.branch, custom_prompts);
            self.candidate_categories
                .insert(sec.trim().to_lowercase(), cat.to_string());
        }
    }

    /// Look up the canonical goal-reaching classification category (`"goal"`, `"plan"`, `"impl"`,
    /// `"verify"`, `"debug"`, `"review"`, `"refactor"`, `"custom"`) for `text`.
    pub fn category_for_text(&self, text: &str) -> &'static str {
        let lower = text.trim().to_lowercase();
        if let Some(cat) = self.candidate_categories.get(&lower) {
            match cat.as_str() {
                "goal" => return "goal",
                "plan" => return "plan",
                "impl" => return "impl",
                "verify" => return "verify",
                "debug" => return "debug",
                "review" => return "review",
                "refactor" => return "refactor",
                "custom" => return "custom",
                _ => {}
            }
        }
        classify_suggestion_for_goal(text, &self.branch, &[])
    }

    /// Look up the localized goal-reaching classification badge (e.g. `"[тест]"` / `"[verify]"`,
    /// `"[цель]"` / `"[goal]"`, `"[свой]"` / `"[custom]"`) for `text`.
    pub fn badge_for_text(&self, text: &str) -> &'static str {
        let cyrillic = detect_cyrillic_language(text);
        format_goal_category_badge(self.category_for_text(text), cyrillic)
    }
}

/// Pure distribution logic for Suggested Replies:
/// - Sanitizes and filters out empty texts or non-finite confidences.
/// - Clamps confidences to `0.0..=1.0` and deduplicates case-insensitively (keeping highest score).
/// - Sorts candidates by `confidence` descending (`total_cmp`).
/// - If the top candidate has `confidence >= confidence_threshold`, promotes it to
///   `default_action` and places up to `max_secondary_suggestions` (`1..=20`) remaining
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
    let max_secondary = max_secondary_suggestions.clamp(1, 20);

    let mut deduplicated: Vec<ScoredSuggestion> = Vec::new();
    for item in candidates {
        let text = item.text.trim();
        if text.is_empty() || !item.confidence.is_finite() {
            continue;
        }
        let confidence = item.confidence.clamp(0.0, 1.0);
        let text_lower = text.to_lowercase();
        if let Some(existing) = deduplicated
            .iter_mut()
            .find(|c| c.text.to_lowercase() == text_lower)
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
            candidate_categories: std::collections::HashMap::new(),
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

    let mut resp = SuggestedRepliesResponse {
        default_action,
        secondary_suggestions,
        branch,
        scored_candidates: deduplicated,
        candidate_categories: std::collections::HashMap::new(),
        debug: None,
    };
    resp.populate_goal_categories(&[]);
    resp
}

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
                "Проверить валидность артефактов OpenSpec (openspec validate --all --strict)".to_string(),
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
                "Проанализировать вывод падающего теста и найти корневую причину ошибки".to_string(),
                "Добавить минимальный регрессионный тест, воспроизводящий сбой".to_string(),
                "Проверить недавний git diff на регрессии в затронутом модуле".to_string(),
                "Запустить проблемный тест изолированно с подробным логом".to_string(),
                "Проверить исправление на граничных случаях и перезапустить тесты".to_string(),
                "Выполнить 6-линзовое ревью (/review 6) для поиска скрытых проблем".to_string(),
            ],
            "review" => vec![
                "Исправить замечания из 6-линзового ревью и перезапустить тесты".to_string(),
                "Запустить /review 6 для аудита корректности, архитектуры, безопасности и тестов".to_string(),
                "Прогнать тестовый набор для подтверждения всех условий ревью".to_string(),
                "Проверить git status и git diff перед коммитом".to_string(),
                "Обновить документацию и артефакты OpenSpec в соответствии с кодом".to_string(),
                "Закоммитить проверенные изменения".to_string(),
            ],
            "architecture" => vec![
                "Приступить к выполнению задач по плану (/opsx:apply)".to_string(),
                "Провести 6-линзовое Go/No-Go ревью предложенной архитектуры".to_string(),
                "Проверить валидность артефактов OpenSpec (openspec validate --all --strict)".to_string(),
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

fn infer_macro_branch_from_text(text: &str) -> &'static str {
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
    } else if lower.contains("refactor")
        || lower.contains("cleanup")
        || lower.contains("рефактор")
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

    if let Some(findings) = six_lens_findings.map(str::trim).filter(|s| !s.is_empty()) {
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
        let mut resp = distribute_suggestions(
            "review",
            candidates,
            confidence_threshold,
            max_secondary,
        );
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
        let _ = HTTP_NO_PROXY_CLIENT.set(built);
        Ok(HTTP_NO_PROXY_CLIENT.get().expect("client initialized"))
    } else {
        if let Some(c) = HTTPS_CLIENT.get() {
            return Ok(c);
        }
        let built = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(4))
            .timeout(std::time::Duration::from_secs(12))
            .build()
            .context("failed to build OpenGF HTTPS client")?;
        let _ = HTTPS_CLIENT.set(built);
        Ok(HTTPS_CLIENT.get().expect("client initialized"))
    }
}

async fn complete_via_named_openai_compatible(
    name: &str,
    profile: &crate::config::NamedProviderConfig,
    system: &str,
    user_msg: &str,
) -> Result<(String, String)> {
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
            std::env::var(env_key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
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

    let client = open_gf_http_client(url.starts_with("http://"))?;
    let mut req = client.post(&url).json(&json!({
        "model": model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user_msg }
        ],
        "max_tokens": 900,
        "temperature": 0.2,
        "stream": false
    }));
    if let Some(token) = key {
        req = req.bearer_auth(token);
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
    let system = "You are OpenGF, the generative candidate synthesizer for Jcode's two-stage SystemOne Suggested Replies cascade. Return ONLY valid JSON with the shape: {\"branch\": \"implementation|verification|debugging|review|architecture|refactoring\", \"candidates\": [\"short actionable reply 1\", \"short actionable reply 2\", ...]}. Generate concise, complete, self-contained next-step user replies (up to 120 chars each, never cut mid-sentence) tailored to the conversation state, extracted assistant options, and pending todos. CRITICAL: Write every candidate in the EXACT SAME natural language used by the user in their most recent message (e.g. Russian if the user wrote in Russian, English if in English). Order candidates from the most likely immediate next user reply (#1) to lower-priority alternatives.";

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
        let mut resp = score_locally_with_context(
            ctx,
            None,
            &custom_prompts,
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
            let is_auto = config.jev_provider.trim().eq_ignore_ascii_case("auto");
            let primary_is_laya = primary.provider_name() == "laya";
            clients_to_try.push((
                format!("{}:{}", config.jev_provider, primary.provider_name()),
                primary,
            ));
            if is_auto
                && !primary_is_laya
                && crate::jev::JevClient::has_configured_laya_route()
                && let Ok(laya_client) =
                    crate::jev::JevClient::for_suggestions_with_selector("laya")
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
            let q = build_macro_branch_choice_question();
            match client.evaluate(iter1_state, q).await {
                Ok(resp) => Ok(resp
                    .pointer("/answers/macro_branch/choice")
                    .and_then(Value::as_str)
                    .unwrap_or("implementation")
                    .to_string()),
                Err(err) => Err(format!("iter1(choice,{selector_label}) error: {err}")),
            }
        } else {
            let q = build_macro_branch_noul_questions();
            match client.evaluate(iter1_state, q).await {
                Ok(resp) => Ok(MACRO_BRANCHES
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
                    .unwrap_or_else(|| "implementation".to_string())),
                Err(err) => Err(format!("iter1(noul,{selector_label}) error: {err}")),
            }
        };

        let branch = match branch_res {
            Ok(b) => b,
            Err(err_msg) => {
                jev_status = err_msg.clone();
                crate::logging::info(&format!(
                    "[suggested_replies] {err_msg}; trying next route or local scorer"
                ));
                continue;
            }
        };

        // Iteration 2: Build candidate pool (OpenGF or contextual + custom_prompts)
        // and evaluate via joint `choice + noul` SystemOne request.
        let from_open_gf = open_gf_result
            .as_ref()
            .is_some_and(|(_, cands)| !cands.is_empty());
        let contextual_pool = contextual_candidates_for_branch(&branch, ctx);
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
        let candidate_texts =
            merge_custom_prompts(base_candidates, &custom_prompts, desired_pool);
        let include_choice =
            client.supports_choice_questions() && candidate_texts.len() >= 2;

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
                                .unwrap_or(0.0)
                                as f32;
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
                    let mut distributed = distribute_suggestions(
                        branch.clone(),
                        scored,
                        threshold,
                        max_secondary,
                    );
                    distributed.populate_goal_categories(&custom_prompts);
                    let top_conf = distributed.top_confidence();
                    jev_status = format!(
                        "ok (selector={selector_label}, mode={}, branch={branch}, iter2={}ms, top={:.2})",
                        if include_choice { "choice+noul" } else { "noul" },
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
    let mut fallback = score_locally_with_context(
        ctx,
        open_gf_result,
        &custom_prompts,
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

/// Execute the full two-stage cascade (backward-compatible entry point taking `transcript_tail`).
pub async fn evaluate_cascade(
    transcript_tail: &str,
    config: &crate::config::SuggestedRepliesConfig,
    six_lens_findings: Option<&str>,
) -> SuggestedRepliesResponse {
    let ctx = SuggestedRepliesContext::from_transcript(transcript_tail);
    evaluate_cascade_with_context(&ctx, config, six_lens_findings).await
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
        assert_eq!(result.confidence_for_text("Fix HTTP 402 fallback"), Some(0.81));
        assert_eq!(result.confidence_for_text("Inspect git diff"), Some(0.63));

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
    fn cascade_outputs_exact_configured_suggestion_counts_for_1_3_and_20() {
        let transcript = "[user]: Проверь тестами, что при настройках 1, 3 и 20 каскад выдаёт нужное число подсказок.\n\n[assistant]: Готово! Что проверим дальше?\n1. Запустить тесты каскада\n2. Проверить TUI рендеринг";
        let ctx = SuggestedRepliesContext::from_transcript(transcript)
            .with_pending_todos(vec!["Проверить лимиты 1, 3 и 20".to_string()]);
        assert!(ctx.language_is_cyrillic);
        assert!(ctx.assistant_options.len() >= 2);

        for expected_count in [1usize, 3usize, 20usize] {
            let resp = score_locally_with_context(
                &ctx,
                None,
                &["Мой постоянный вопрос: проверь статус git".to_string()],
                None,
                0.55,
                expected_count,
            );
            assert!(
                resp.default_action.is_some(),
                "expected default_action for count={expected_count}"
            );
            assert_eq!(
                resp.secondary_suggestions.len(),
                expected_count,
                "cascade must emit exactly {expected_count} secondary suggestions"
            );
            // Every emitted row (default_action and all secondary_suggestions) must have a valid confidence score.
            let def_text = resp.default_action.as_deref().unwrap();
            assert!(resp.confidence_for_text(def_text).is_some());
            for sec in &resp.secondary_suggestions {
                assert!(
                    resp.confidence_for_text(sec).is_some(),
                    "missing confidence for secondary suggestion '{sec}'"
                );
            }
        }
    }

    #[test]
    fn joint_choice_and_noul_questions_and_structured_state_respect_invariants() {
        let ctx = SuggestedRepliesContext::from_transcript(
            "[user]: Добавь две опции в настройки\n\n[assistant]: Выберите вариант:\n1. Показывать уверенность модели\n2. Добавить список постоянных вопросов",
        );
        let cands = contextual_candidates_for_branch("implementation", &ctx);
        assert!(cands.len() >= 20);

        let state = build_structured_systemone_state(Some("implementation"), &ctx, &cands[..4]);
        assert_eq!(state["workflow_branch"], "implementation");
        assert_eq!(state["language"], "ru");
        assert_eq!(state["candidates"]["cand_0"], cands[0]);

        // With >= 2 candidates and include_choice = true, best_candidate (choice) + cand_0..3 (noul) are present.
        let joint_q = build_micro_scoring_questions("implementation", &cands[..4], true).unwrap();
        assert_eq!(joint_q["best_candidate"]["type"], "choice");
        assert_eq!(joint_q["cand_0"]["type"], "noul");
        assert_eq!(joint_q.len(), 5);

        // With 1 candidate, best_candidate is omitted so jev.rs choice validation (2..=255) never fails.
        let single_q = build_micro_scoring_questions("implementation", &cands[..1], true).unwrap();
        assert!(!single_q.contains_key("best_candidate"));
        assert_eq!(single_q.len(), 1);
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
        let top = calibrate_micro_score(0.6089, 0, true);
        assert!(top >= 0.75, "expected calibrated top score >= 0.75, got {top}");
        let top_with_choice = calibrate_micro_score_with_choice(0.8339, Some(0.35), true, 0, true);
        assert!(top_with_choice > top);
        let second = calibrate_micro_score(0.5864, 1, true);
        assert!(top > second);
        assert_eq!(calibrate_micro_score(0.42, 0, true), 0.42);
    }

    #[tokio::test]
    async fn complete_via_named_openai_compatible_accepts_literal_sk_in_api_key_env() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(
                    req.to_ascii_lowercase()
                        .contains("authorization: bearer sk-test-inline-secret"),
                    "missing bearer header in {req}"
                );
                let body = r#"{"choices":[{"message":{"content":"{\"branch\":\"debugging\",\"candidates\":[\"Проверить отладку через /suggestions debug\",\"Запустить тесты\"]}"}}]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let profile = crate::config::NamedProviderConfig {
            base_url: format!("http://{addr}/v1"),
            api_key_env: Some("sk-test-inline-secret".to_string()),
            default_model: Some("gpt-5.6-luna".to_string()),
            ..Default::default()
        };
        let (raw, label) =
            complete_via_named_openai_compatible("clipapi", &profile, "sys", "usr")
                .await
                .unwrap();
        assert_eq!(label, "profile:clipapi:gpt-5.6-luna");
        let (branch, cands) = parse_open_gf_response(&raw).unwrap();
        assert_eq!(branch, "debugging");
        assert_eq!(cands[0], "Проверить отладку через /suggestions debug");
    }

    #[test]
    fn suggested_replies_env_overrides_apply_count_confidence_and_custom_prompts() {
        let _lock = crate::storage::lock_test_env();
        let keys = [
            "JCODE_SUGGESTED_REPLIES_MAX_SECONDARY",
            "JCODE_MAX_SECONDARY_SUGGESTIONS",
            "JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE",
            "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
        ];
        let saved: Vec<(&str, Option<std::ffi::OsString>)> =
            keys.iter().map(|k| (*k, std::env::var_os(k))).collect();

        crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "15");
        crate::env::set_var("JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE", "false");
        crate::env::set_var(
            "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
            "Вопрос 1 | Вопрос 2 ; Вопрос 3",
        );

        let mut cfg = crate::config::Config::default();
        cfg.apply_env_overrides();
        assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 15);
        assert!(!cfg.suggested_replies.show_confidence);
        assert_eq!(
            cfg.suggested_replies.sanitized_custom_prompts(),
            vec!["Вопрос 1", "Вопрос 2", "Вопрос 3"]
        );

        // JSON array format + clamping out-of-range counts (99 -> 20, -4 -> 1).
        crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "99");
        crate::env::set_var("JCODE_SUGGESTED_REPLIES_SHOW_CONFIDENCE", "1");
        crate::env::set_var(
            "JCODE_SUGGESTED_REPLIES_CUSTOM_PROMPTS",
            r#"["JSON вопрос A", "JSON вопрос B", ""]"#,
        );
        cfg.apply_env_overrides();
        assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 20);
        assert!(cfg.suggested_replies.show_confidence);
        assert_eq!(
            cfg.suggested_replies.sanitized_custom_prompts(),
            vec!["JSON вопрос A", "JSON вопрос B"]
        );

        crate::env::set_var("JCODE_SUGGESTED_REPLIES_MAX_SECONDARY", "-4");
        cfg.apply_env_overrides();
        assert_eq!(cfg.suggested_replies.max_secondary_suggestions, 1);

        for (k, val) in saved {
            match val {
                Some(v) => crate::env::set_var(k, v),
                None => crate::env::remove_var(k),
            }
        }
    }

    #[test]
    fn extract_assistant_options_handles_code_fences_bullets_and_slash_commands() {
        let msg = r#"Вот пример кода:
```bash
1. Fake numbered item inside code fence
- Fake bullet inside code fence
`/fake-inside-fence`
```
Что делаем дальше?
- Проверить конфигурацию в `crates/jcode-base/src/config.rs`
• Запустить модульные тесты — убедиться, что всё зелёное
1) Сделать коммит изменений
Также можно вызвать `/opsx:apply` или `/review 6`, но не путь `/usr/local/bin/cargo` и не `https://example.com/test`.
"#;
        let opts = extract_assistant_options(msg);
        assert!(
            !opts.iter().any(|o| o.contains("Fake")),
            "must ignore lines inside fenced code blocks: {opts:?}"
        );
        assert!(
            !opts.iter().any(|o| o.contains("/usr/local")),
            "must ignore file paths: {opts:?}"
        );
        assert!(
            opts.iter()
                .any(|o| o.contains("Проверить конфигурацию")),
            "must extract '-' bullet option: {opts:?}"
        );
        assert!(
            opts.iter()
                .any(|o| o.contains("Запустить модульные тесты")),
            "must extract '•' bullet option and strip em-dash explanation: {opts:?}"
        );
        assert!(
            opts.iter().any(|o| o == "Сделать коммит изменений"),
            "must extract '1)' numbered option: {opts:?}"
        );
        assert!(
            opts.iter().any(|o| o == "/opsx:apply") && opts.iter().any(|o| o == "/review 6"),
            "must extract backticked slash commands: {opts:?}"
        );
    }

    #[tokio::test]
    async fn evaluate_cascade_with_context_executes_two_stage_laya_systemone_and_falls_back_on_http_402() {
        use std::io::{Read, Write};
        let _lock = crate::storage::lock_test_env();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let server_thread = std::thread::spawn(move || {
            // Request 1: Iteration 1 macro_branch choice
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 16384];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                assert!(req.contains("\"macro_branch\""), "expected macro_branch question in iter1: {req}");
                let body = r#"{"id":"dec_1","model":"laya-latest","answers":{"macro_branch":{"type":"choice","choice":"verification","probabilities":{"verification":0.82,"implementation":0.18}}}}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }

            // Request 2: Iteration 2 joint best_candidate (choice) + cand_0..cand_N (noul)
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 32768];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let json_start = req.find('{').expect("request JSON body");
                let parsed: Value = serde_json::from_str(&req[json_start..]).expect("valid request JSON");
                assert_eq!(parsed["state"]["workflow_branch"], "verification");
                assert_eq!(parsed["questions"]["best_candidate"]["type"], "choice");

                let questions = parsed["questions"].as_object().unwrap();
                let mut answers = Map::new();
                let mut probs = Map::new();
                for key in questions.keys() {
                    if key == "best_candidate" {
                        continue;
                    }
                    let is_winner = key == "cand_1";
                    probs.insert(key.clone(), json!(if is_winner { 0.65 } else { 0.05 }));
                    answers.insert(
                        key.clone(),
                        json!({ "type": "noul", "noul": if is_winner { 0.84 } else { 0.52 } }),
                    );
                }
                answers.insert(
                    "best_candidate".to_string(),
                    json!({
                        "type": "choice",
                        "choice": "cand_1",
                        "probabilities": probs,
                    }),
                );
                let body = serde_json::to_string(&json!({
                    "id": "dec_2",
                    "model": "laya-latest",
                    "answers": answers,
                }))
                .unwrap();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }

            // Request 3: Simulate HTTP 402 Payment Required / error to verify local fallback
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 16384];
                let _ = stream.read(&mut buf);
                let body = r#"{"error":{"message":"Insufficient credits"}}"#;
                let resp = format!(
                    "HTTP/1.1 402 Payment Required\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let saved_laya_url = std::env::var_os("JCODE_LAYA_BASE_URL");
        crate::env::set_var("JCODE_LAYA_BASE_URL", format!("http://{addr}/v1/systemone"));

        let ctx = SuggestedRepliesContext::from_transcript(
            "[user]: Проверь работу каскада через Laya\n\n[assistant]: Выберите шаг:\n1. Первый шаг\n2. Второй шаг (победитель)",
        )
        .with_pending_todos(vec!["Запустить финальные тесты".to_string()]);

        let cfg = crate::config::SuggestedRepliesConfig {
            enabled: true,
            jev_provider: "laya".to_string(),
            use_open_gf: false,
            max_secondary_suggestions: 3,
            confidence_threshold: 0.55,
            show_confidence: true,
            custom_prompts: vec!["Кастомный вопрос".to_string()],
            enable_full_auto_explorer: false,
            six_lens_auto_review: false,
        };

        // 1. Happy path: 2-stage SystemOne evaluation (choice + joint choice+noul)
        let resp = evaluate_cascade_with_context(&ctx, &cfg, None).await;
        assert_eq!(resp.branch, "verification");
        assert_eq!(
            resp.default_action.as_deref(),
            Some("Второй шаг (победитель)"),
            "cand_1 chosen by best_candidate + noul must be promoted to default_action"
        );
        assert_eq!(resp.secondary_suggestions.len(), 3);
        let debug = resp.debug.as_ref().expect("debug info present");
        assert_eq!(debug.source, "fixed+jev");
        assert!(debug.jev_status.contains("mode=choice+noul"));

        // 2. HTTP 402 fallback path: gracefully degrades to local scorer
        let fallback_resp = evaluate_cascade_with_context(&ctx, &cfg, None).await;
        assert!(fallback_resp.default_action.is_some());
        assert_eq!(fallback_resp.secondary_suggestions.len(), 3);
        assert_eq!(
            fallback_resp.debug.as_ref().unwrap().source,
            "fixed+local"
        );

        // 3. Six-lens findings short-circuit path
        let six_lens_resp =
            evaluate_cascade_with_context(&ctx, &cfg, Some("Устранить гонку в кэше")).await;
        assert_eq!(six_lens_resp.branch, "review");
        assert_eq!(
            six_lens_resp.debug.as_ref().unwrap().source,
            "six_lens"
        );
        assert!(
            six_lens_resp
                .default_action
                .as_deref()
                .unwrap()
                .contains("Устранить гонку в кэше")
        );

        match saved_laya_url {
            Some(v) => crate::env::set_var("JCODE_LAYA_BASE_URL", v),
            None => crate::env::remove_var("JCODE_LAYA_BASE_URL"),
        }
        let _ = server_thread.join();
    }

    #[test]
    fn goal_classification_labels_and_badges_cover_all_categories_bilingually() {
        let custom = vec!["Кастомный вопрос для цели".to_string()];
        assert_eq!(
            classify_suggestion_for_goal("Кастомный вопрос для цели", "implementation", &custom),
            "custom"
        );
        assert_eq!(format_goal_category_badge("custom", true), "[свой]");
        assert_eq!(format_goal_category_badge("custom", false), "[custom]");

        assert_eq!(
            classify_suggestion_for_goal("Закоммитить проверенные изменения и завершить цель", "goal_delivery", &[]),
            "goal"
        );
        assert_eq!(format_goal_category_badge("goal", true), "[цель]");
        assert_eq!(format_goal_category_badge("goal", false), "[goal]");

        assert_eq!(
            classify_suggestion_for_goal("Выполнить следующую задачу: обновить конфиг", "planning", &[]),
            "plan"
        );
        assert_eq!(format_goal_category_badge("plan", true), "[план]");
        assert_eq!(format_goal_category_badge("plan", false), "[plan]");

        assert_eq!(
            classify_suggestion_for_goal("Запустить целевые тесты и убедиться, что все проверки проходят", "verification", &[]),
            "verify"
        );
        assert_eq!(format_goal_category_badge("verify", true), "[тест]");
        assert_eq!(format_goal_category_badge("verify", false), "[verify]");

        assert_eq!(
            classify_suggestion_for_goal("Проанализировать вывод падающего теста и найти корневую причину ошибки", "debugging", &[]),
            "debug"
        );
        assert_eq!(format_goal_category_badge("debug", true), "[баг]");
        assert_eq!(format_goal_category_badge("debug", false), "[debug]");

        assert_eq!(
            classify_suggestion_for_goal("Запустить /review 6 для аудита", "review", &[]),
            "review"
        );
        assert_eq!(format_goal_category_badge("review", true), "[ревью]");
        assert_eq!(format_goal_category_badge("review", false), "[review]");

        assert_eq!(
            classify_suggestion_for_goal("Удалить неиспользуемый код и лишние импорты", "refactoring", &[]),
            "refactor"
        );
        assert_eq!(format_goal_category_badge("refactor", true), "[рефактор]");
        assert_eq!(format_goal_category_badge("refactor", false), "[refactor]");

        assert_eq!(
            classify_suggestion_for_goal("Пересобрать и перезагрузить бинарник через selfdev build-reload", "implementation", &[]),
            "impl"
        );
        assert_eq!(format_goal_category_badge("impl", true), "[код]");
        assert_eq!(format_goal_category_badge("impl", false), "[impl]");
    }
}
