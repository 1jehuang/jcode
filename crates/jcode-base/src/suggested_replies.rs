//! Two-stage cascading Suggested Replies with Default Action Focus.
//!
//! Combines OpenJev / Laya SystemOne (`/v1/systemone`, calibrated logit decisions
//! via joint `choice` and `noul` over a structured JSON `state`) with OpenGF
//! (`Sidecar` / named profile generative candidate synthesis with `"store": false`),
//! assistant question/option extraction, user-defined constant questions (`custom_prompts`),
//! optional feature-completion classification (`"finish"` / `[финал]` / `[finish]` via
//! `enable_finish_category`), and a language-aware deterministic local fallback.

use serde::{Deserialize, Serialize};

/// Maximum number of candidates evaluated in a single Iteration 2 micro-scoring batch.
/// Allows 1 Default Action + up to 20 secondary suggestions (`21` candidates total),
/// keeping `1 (best_candidate choice) + 21 (cand_0..cand_20 noul) = 22 <= MAX_QUESTIONS (24)`.
pub const MAX_MICRO_CANDIDATES: usize = 21;

/// Macro conversation branches for Iteration 1 classification toward reaching the user's goal.
pub const MACRO_BRANCHES: &[(&str, &str)] = &[
    (
        "feature_completion",
        "Moving the feature to its successful final completion, acceptance verification, and closure",
    ),
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
/// - `"finish"`: Moves directly toward the successful final completion of the feature (`[финал]` / `[finish]`).
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
    ("finish", "[финал]", "[finish]"),
];

/// Classify how a candidate suggestion advances the user toward reaching their goal
/// or achieving the successful final completion of the feature, using the active config
/// (`suggested_replies.enable_finish_category`).
pub fn classify_suggestion_for_goal(
    text: &str,
    branch: &str,
    custom_prompts: &[String],
) -> &'static str {
    classify_suggestion_for_goal_with_options(
        text,
        branch,
        custom_prompts,
        crate::config::config()
            .suggested_replies
            .enable_finish_category,
    )
}

/// Classify how a candidate suggestion advances the user toward reaching their goal,
/// with an explicit `enable_finish_category` toggle for the optional `"finish"` (`[финал]` / `[finish]`) category.
pub fn classify_suggestion_for_goal_with_options(
    text: &str,
    branch: &str,
    custom_prompts: &[String],
    enable_finish_category: bool,
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

    if lower.contains("завершить фичу")
        || lower.contains("завершение фичи")
        || lower.contains("финальное завершение")
        || lower.contains("финального завершения фичи")
        || lower.contains("успешного финального завершения")
        || lower.contains("закрыть фичу")
        || lower.contains("довести фичу до конца")
        || lower.contains("финализировать фичу")
        || lower.contains("сдать фичу")
        || lower.contains("complete the feature")
        || lower.contains("feature completion")
        || lower.contains("final completion of the feature")
        || lower.contains("successful final completion")
        || lower.contains("finalize the feature")
        || lower.contains("finish the feature")
    {
        return if enable_finish_category {
            "finish"
        } else {
            "goal"
        };
    }

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
        "finish" | "feature_completion" | "final_completion" => {
            if enable_finish_category {
                "finish"
            } else {
                "goal"
            }
        }
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

/// Classify whether a candidate event/action directly moves toward achieving the
/// successful final completion of the feature (`true`) versus exploratory/informational
/// inspection (`false`).
pub fn advances_toward_feature_completion(text: &str, branch: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lower = trimmed.to_lowercase();
    if lower.starts_with("/suggestions debug")
        || lower.starts_with("/suggestions status")
        || lower.starts_with("/help")
        || lower.contains("изучить границы модулей")
        || lower.contains("inspect module boundaries")
    {
        return false;
    }
    let category = classify_suggestion_for_goal(trimmed, branch, &[]);
    if matches!(category, "finish" | "goal" | "verify" | "review") {
        return true;
    }
    lower.contains("исправ")
        || lower.contains("устран")
        || lower.contains("fix")
        || lower.contains("resolve")
        || lower.contains("выполнить следующую задачу")
        || lower.contains("продолжить реализацию")
        || lower.contains("приступить к выполнению")
        || lower.contains("обновить чекбоксы")
        || lower.contains("закоммит")
        || lower.contains("коммит")
        || lower.contains("реализ")
        || lower.contains("пересобрать")
        || lower.contains("build-reload")
        || lower.contains("work on next todo")
        || lower.contains("continue implementing")
        || lower.contains("apply the openspec")
        || lower.contains("update openspec")
        || lower.contains("implement")
        || lower.contains("commit")
        || lower.contains("deliver")
        || lower.contains("complete")
        || lower.contains("finalize")
        || matches!(
            branch.trim().to_ascii_lowercase().as_str(),
            "finish" | "feature_completion" | "final_completion" | "goal_delivery" | "verification"
        )
}

/// Format a canonical goal classification category (`"goal"`, `"finish"`, `"plan"`, `"impl"`,
/// `"verify"`, `"debug"`, `"review"`, `"refactor"`, `"custom"`) as a localized compact UI badge.
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
        "finish" | "feature_completion" | "final_completion" => 8,
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

        let assistant_source = last_assistant_message.as_deref().unwrap_or(transcript_tail);
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

/// Truncate `text` to at most `max_bytes` while preserving the closing tail on clean
/// line/word boundaries (and without inserting literal `...` markers that LLMs mistake
/// for truncated tool or assistant output).
pub fn truncate_preserving_tail(text: &str, max_bytes: usize) -> String {
    let trimmed = text.trim();
    if trimmed.len() <= max_bytes {
        return trimmed.to_string();
    }
    let head_budget = max_bytes / 3;
    let tail_budget = max_bytes.saturating_sub(head_budget);
    let mut head_end = head_budget.min(trimmed.len());
    while head_end > 0 && !trimmed.is_char_boundary(head_end) {
        head_end -= 1;
    }
    if let Some(nl) = trimmed[..head_end].rfind('\n')
        && nl >= head_end / 2
    {
        head_end = nl;
    } else if let Some(sp) = trimmed[..head_end].rfind(char::is_whitespace)
        && sp >= head_end / 2
    {
        head_end = sp;
    }

    let mut tail_start = trimmed.len().saturating_sub(tail_budget);
    while tail_start < trimmed.len() && !trimmed.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let tail_slice = &trimmed[tail_start..];
    if let Some(nl) = tail_slice.find('\n')
        && nl <= tail_slice.len() / 3
    {
        tail_start += nl + 1;
    } else if let Some((sp_idx, sp_ch)) = tail_slice.char_indices().find(|(_, c)| c.is_whitespace())
        && sp_idx <= tail_slice.len() / 3
    {
        tail_start += sp_idx + sp_ch.len_utf8();
    }

    if head_end >= tail_start {
        return trimmed.to_string();
    }
    format!(
        "{}\n\n{}",
        trimmed[..head_end].trim_end(),
        trimmed[tail_start..].trim_start()
    )
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

/// Extract actionable findings or a `DEFAULT_ACTION:` directive from a 6-Lens Review reply.
pub fn extract_six_lens_review_findings(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty()
        || trimmed.contains("You are the 6-Lens Subagent Self-Review coordinator")
        || trimmed.contains("prefixed with DEFAULT_ACTION: describing the exact fix")
    {
        return None;
    }

    for raw_line in trimmed.lines() {
        let line = raw_line
            .trim()
            .trim_start_matches(['-', '*', '•', '>', ' '])
            .trim_start_matches(['*', '`', ' ']);
        let prefix = "default_action:";
        if line
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            let action = line[prefix.len()..]
                .trim()
                .trim_matches(['*', '`', '"', '\''])
                .trim();
            if !action.is_empty()
                && !matches!(
                    action.to_ascii_lowercase().as_str(),
                    "none" | "n/a" | "no action needed"
                )
            {
                return Some(action.to_string());
            }
        }
    }

    let lower = trimmed.to_lowercase();
    let is_review_report = (lower.contains("overall verdict")
        || lower.contains("6-lens")
        || lower.contains("6-линз")
        || lower.contains("lens 1"))
        && (lower.contains("no-go")
            || lower.contains("conditional go")
            || lower.contains("conditional-go"));
    if is_review_report {
        for raw_line in trimmed.lines() {
            let line = raw_line.trim();
            let item = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| line.strip_prefix("• "));
            if let Some(item) = item {
                let cleaned = clean_extracted_option(item);
                let item_lower = cleaned.to_lowercase();
                if !cleaned.is_empty()
                    && !item_lower.starts_with("overall verdict")
                    && !item_lower.starts_with("lens ")
                {
                    return Some(cleaned);
                }
            }
        }
    }

    None
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
    let all_lines: Vec<&str> = trimmed.lines().collect();
    let tail_start = all_lines.len().saturating_sub(28);

    for (idx, raw_line) in all_lines.into_iter().enumerate() {
        let line = raw_line.trim();
        if line.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if idx < tail_start || in_code_block || line.is_empty() {
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

fn is_status_report_label(label: &str) -> bool {
    let lower = label.trim().to_lowercase();
    lower.contains("(go)")
        || lower.contains("(no-go)")
        || lower.contains("conditional go")
        || matches!(
            lower.as_str(),
            "config"
                | "ui state"
                | "controls"
                | "source"
                | "opengf"
                | "jev / laya"
                | "structured context"
                | "default action"
                | "scored candidates"
                | "security"
                | "architecture"
                | "performance"
                | "maintainability"
                | "bugs & edge cases"
                | "ux"
                | "openspec"
                | "статус"
                | "итог"
                | "ограничение"
                | "ограничение по апстриму"
        )
        || lower.starts_with("lens ")
        || lower.starts_with("линз")
        || lower.starts_with("встроенная документация")
        || lower.starts_with("исправлена ")
        || lower.starts_with("обрезка ")
        || lower.starts_with("исключение ")
        || lower.starts_with("защита от ")
}

fn clean_extracted_option(raw: &str) -> String {
    let raw_trimmed = raw.trim();
    let without_bold = raw_trimmed.replace("**", "");
    let trimmed = without_bold
        .trim()
        .trim_matches(|c: char| c == '`' || c == '"' || c == '«' || c == '»')
        .trim_end_matches([';', ',']);
    let head = if let Some((left, _)) = trimmed.split_once(" — ") {
        left.trim()
    } else if let Some((left, _)) = trimmed.split_once(": ") {
        let left_trimmed = left.trim();
        let left_chars = left_trimmed.chars().count();
        if (10..=80).contains(&left_chars) && !is_status_report_label(left_trimmed) {
            left_trimmed
        } else {
            return String::new();
        }
    } else {
        trimmed
    };
    head.trim().to_string()
}

fn is_viable_option_candidate(text: &str) -> bool {
    let char_count = text.chars().count();
    if !(4..=96).contains(&char_count)
        || text.starts_with("http://")
        || text.starts_with("https://")
        || text.starts_with('[')
        || text.ends_with("...")
        || text.ends_with('…')
        || text.ends_with(':')
        || text.contains("...")
        || text.contains('…')
    {
        return false;
    }
    let lower = text.to_lowercase();
    !lower.contains("(truncated)")
        && !lower.contains("output truncated")
        && !lower.contains("обрезан")
        && !lower.contains("(go)")
        && !lower.contains("(no-go)")
        && !lower.contains("conditional go")
        && !lower.contains("mergeable")
        && !lower.contains("confidence score")
}

pub(super) fn push_unique_case_insensitive(list: &mut Vec<String>, candidate: String) {
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
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub candidate_feature_completion: std::collections::HashMap<String, bool>,
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

    /// Populate `candidate_categories` and `candidate_feature_completion` for all candidates,
    /// tagging any match in `custom_prompts` as `"custom"`.
    pub fn populate_goal_categories(&mut self, custom_prompts: &[String]) {
        self.populate_goal_categories_with_options(
            custom_prompts,
            crate::config::config()
                .suggested_replies
                .enable_finish_category,
        );
    }

    /// Populate `candidate_categories` and `candidate_feature_completion` with an explicit
    /// `enable_finish_category` toggle.
    pub fn populate_goal_categories_with_options(
        &mut self,
        custom_prompts: &[String],
        enable_finish_category: bool,
    ) {
        if !enable_finish_category
            && matches!(
                self.branch.trim().to_ascii_lowercase().as_str(),
                "finish" | "feature_completion" | "final_completion"
            )
        {
            self.branch = "goal_delivery".to_string();
        }
        for cand in &self.scored_candidates {
            let key = cand.text.trim().to_lowercase();
            let cat = classify_suggestion_for_goal_with_options(
                &cand.text,
                &self.branch,
                custom_prompts,
                enable_finish_category,
            );
            let fin = advances_toward_feature_completion(&cand.text, &self.branch);
            self.candidate_categories
                .insert(key.clone(), cat.to_string());
            self.candidate_feature_completion.insert(key, fin);
        }
        if let Some(ref def) = self.default_action {
            let key = def.trim().to_lowercase();
            let cat = classify_suggestion_for_goal_with_options(
                def,
                &self.branch,
                custom_prompts,
                enable_finish_category,
            );
            let fin = advances_toward_feature_completion(def, &self.branch);
            self.candidate_categories
                .insert(key.clone(), cat.to_string());
            self.candidate_feature_completion.insert(key, fin);
        }
        for sec in &self.secondary_suggestions {
            let key = sec.trim().to_lowercase();
            let cat = classify_suggestion_for_goal_with_options(
                sec,
                &self.branch,
                custom_prompts,
                enable_finish_category,
            );
            let fin = advances_toward_feature_completion(sec, &self.branch);
            self.candidate_categories
                .insert(key.clone(), cat.to_string());
            self.candidate_feature_completion.insert(key, fin);
        }
    }

    /// Look up the canonical goal-reaching classification category (`"goal"`, `"finish"`, `"plan"`,
    /// `"impl"`, `"verify"`, `"debug"`, `"review"`, `"refactor"`, `"custom"`) for `text`.
    pub fn category_for_text(&self, text: &str) -> &'static str {
        let finish_enabled = crate::config::config()
            .suggested_replies
            .enable_finish_category;
        let lower = text.trim().to_lowercase();
        if let Some(cat) = self.candidate_categories.get(&lower) {
            match cat.as_str() {
                "finish" | "feature_completion" | "final_completion" => {
                    return if finish_enabled { "finish" } else { "goal" };
                }
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
        classify_suggestion_for_goal_with_options(text, &self.branch, &[], finish_enabled)
    }

    /// True when `text` is classified as advancing toward the successful final completion of the feature.
    pub fn advances_feature_completion_for_text(&self, text: &str) -> bool {
        let lower = text.trim().to_lowercase();
        if let Some(&advances) = self.candidate_feature_completion.get(&lower) {
            return advances;
        }
        advances_toward_feature_completion(text, &self.branch)
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
            candidate_feature_completion: std::collections::HashMap::new(),
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
        candidate_feature_completion: std::collections::HashMap::new(),
        debug: None,
    };
    resp.populate_goal_categories(&[]);
    resp
}

mod candidates;
mod cascade;
#[cfg(test)]
mod tests;

pub use candidates::{
    build_macro_branch_choice_question, build_macro_branch_choice_question_with_options,
    build_macro_branch_noul_questions, build_macro_branch_noul_questions_with_options,
    build_micro_scoring_noul_questions, build_micro_scoring_questions,
    build_structured_systemone_state, contextual_candidates_for_branch,
    contextual_candidates_for_branch_with_options, fixed_candidates_for_branch,
    merge_custom_prompts, score_locally_from_context, score_locally_with_context,
    score_locally_with_context_and_options,
};
pub use cascade::{
    calibrate_micro_score, calibrate_micro_score_with_choice, evaluate_cascade,
    evaluate_cascade_with_context, generate_open_gf_candidates,
    generate_open_gf_candidates_for_context, generate_open_gf_candidates_with_route,
    parse_open_gf_response,
};
