use jcode_message_types::ToolCall;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderedMessage {
    pub role: String,
    pub content: String,
    pub tool_calls: Vec<String>,
    pub tool_data: Option<ToolCall>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderedCompactedHistoryInfo {
    pub total_messages: usize,
    pub visible_messages: usize,
    pub remaining_messages: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderedImageSource {
    UserInput,
    ToolResult { tool_name: String },
    Other { role: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenderedImage {
    pub media_type: String,
    pub data: String,
    pub label: Option<String>,
    pub source: RenderedImageSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum SessionStatus {
    #[default]
    Active,
    Closed,
    Crashed {
        message: Option<String>,
    },
    Reloaded,
    Compacted,
    RateLimited,
    Error {
        message: String,
    },
}

impl SessionStatus {
    pub fn display(&self) -> &'static str {
        match self {
            SessionStatus::Active => "active",
            SessionStatus::Closed => "closed",
            SessionStatus::Crashed { .. } => "crashed",
            SessionStatus::Reloaded => "reloaded",
            SessionStatus::Compacted => "compacted",
            SessionStatus::RateLimited => "rate limited",
            SessionStatus::Error { .. } => "error",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            SessionStatus::Active => "▶",
            SessionStatus::Closed => "✓",
            SessionStatus::Crashed { .. } => "💥",
            SessionStatus::Reloaded => "🔄",
            SessionStatus::Compacted => "📦",
            SessionStatus::RateLimited => "⏳",
            SessionStatus::Error { .. } => "❌",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            SessionStatus::Crashed { message } => message.as_deref(),
            SessionStatus::Error { message } => Some(message.as_str()),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SessionImproveMode {
    #[serde(rename = "improve_run", alias = "run")]
    ImproveRun,
    #[serde(rename = "improve_plan", alias = "plan")]
    ImprovePlan,
    #[serde(rename = "refactor_run")]
    RefactorRun,
    #[serde(rename = "refactor_plan")]
    RefactorPlan,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitState {
    pub root: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvSnapshot {
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub reason: String,
    pub session_id: String,
    pub working_dir: Option<String>,
    pub provider: String,
    pub model: String,
    pub jcode_version: String,
    pub jcode_git_hash: Option<String>,
    pub jcode_git_dirty: Option<bool>,
    pub os: String,
    pub arch: String,
    pub pid: u32,
    pub is_selfdev: bool,
    pub is_debug: bool,
    pub is_canary: bool,
    pub testing_build: Option<String>,
    pub working_git: Option<GitState>,
}

#[derive(Debug, Clone)]
pub struct SessionSearchQueryProfile {
    pub normalized: String,
    pub terms: Vec<String>,
    pub min_term_matches: usize,
}

impl SessionSearchQueryProfile {
    pub fn new(query: &str) -> Self {
        let normalized = query.trim().to_lowercase();
        let terms = tokenize_session_search_query(&normalized);
        let min_term_matches = minimum_session_search_term_matches(terms.len());
        Self {
            normalized,
            terms,
            min_term_matches,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.normalized.is_empty()
    }

    pub fn is_actionable(&self) -> bool {
        !self.normalized.is_empty() && !self.terms.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct SessionSearchMatchScore {
    pub snippet: String,
    pub score: f64,
    pub matched_terms: Vec<String>,
    pub exact_match: bool,
}

pub fn score_session_search_text_match(
    text: &str,
    query: &SessionSearchQueryProfile,
) -> Option<SessionSearchMatchScore> {
    if !query.is_actionable() {
        return None;
    }

    let text_lower = text.to_lowercase();
    let exact_pos = (!query.normalized.is_empty())
        .then(|| text_lower.find(&query.normalized))
        .flatten();

    let mut matched_terms = Vec::new();
    let mut total_term_hits = 0usize;
    let mut first_term_pos = None;

    for term in &query.terms {
        if let Some(pos) = text_lower.find(term) {
            matched_terms.push(term.clone());
            total_term_hits += text_lower.matches(term).count();
            first_term_pos = Some(first_term_pos.map_or(pos, |current: usize| current.min(pos)));
        }
    }

    if exact_pos.is_none() && matched_terms.len() < query.min_term_matches {
        return None;
    }

    let anchor = exact_pos.or(first_term_pos);
    let snippet = extract_session_search_snippet(text, anchor, query, 280);
    let coverage = matched_terms.len() as f64 / query.terms.len() as f64;
    let score = if exact_pos.is_some() { 4.0 } else { 0.0 }
        + coverage * 3.0
        + matched_terms.len() as f64 * 0.25
        + (total_term_hits as f64 / (text.len() as f64 + 1.0)) * 200.0;

    Some(SessionSearchMatchScore {
        snippet,
        score,
        matched_terms,
        exact_match: exact_pos.is_some(),
    })
}

pub fn session_search_raw_matches_query(raw: &[u8], query: &SessionSearchQueryProfile) -> bool {
    if !query.is_actionable() {
        return false;
    }

    if query.normalized.is_ascii() {
        if contains_case_insensitive_bytes(raw, query.normalized.as_bytes()) {
            return true;
        }
        let matched_terms = query
            .terms
            .iter()
            .filter(|term| contains_case_insensitive_bytes(raw, term.as_bytes()))
            .count();
        return matched_terms >= query.min_term_matches;
    }

    let Ok(raw_text) = std::str::from_utf8(raw) else {
        return false;
    };
    normalized_session_search_text_matches(&raw_text.to_lowercase(), query)
}

pub fn session_search_path_matches_query(
    path_text: &str,
    query: &SessionSearchQueryProfile,
) -> bool {
    normalized_session_search_text_matches(&path_text.to_lowercase(), query)
}

pub fn normalized_session_search_text_matches(
    text_lower: &str,
    query: &SessionSearchQueryProfile,
) -> bool {
    if !query.is_actionable() {
        return false;
    }
    if text_lower.contains(&query.normalized) {
        return true;
    }
    query
        .terms
        .iter()
        .filter(|term| text_lower.contains(term.as_str()))
        .count()
        >= query.min_term_matches
}

pub fn tokenize_session_search_query(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = HashSet::new();

    for token in query.split(|c: char| !c.is_alphanumeric()) {
        if token.is_empty() {
            continue;
        }

        let token = token.to_lowercase();
        if is_session_search_stop_word(&token) {
            continue;
        }

        let keep = token.chars().count() >= 2 || token.chars().all(|c| c.is_ascii_digit());
        if keep && seen.insert(token.clone()) {
            terms.push(token);
        }
    }

    terms
}

pub fn is_session_search_stop_word(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "at"
            | "be"
            | "but"
            | "by"
            | "for"
            | "from"
            | "how"
            | "i"
            | "in"
            | "into"
            | "is"
            | "it"
            | "my"
            | "of"
            | "on"
            | "or"
            | "our"
            | "that"
            | "the"
            | "their"
            | "this"
            | "to"
            | "we"
            | "what"
            | "when"
            | "where"
            | "which"
            | "with"
            | "you"
            | "your"
    )
}

pub fn minimum_session_search_term_matches(term_count: usize) -> usize {
    match term_count {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=5 => 2,
        _ => 3,
    }
}

/// Fast case-insensitive byte search. Avoids allocating a lowercase copy of the
/// entire file for the common ASCII-query case.
pub fn contains_case_insensitive_bytes(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.len() < needle_lower.len() {
        return false;
    }
    let end = haystack.len() - needle_lower.len();
    'outer: for i in 0..=end {
        for (j, &nb) in needle_lower.iter().enumerate() {
            let hb = haystack[i + j];
            let hb_lower = if hb.is_ascii_uppercase() {
                hb | 0x20
            } else {
                hb
            };
            if hb_lower != nb {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

pub fn session_search_working_dir_matches(session_wd: &str, filter: &str) -> bool {
    let session_norm = normalize_path_for_session_search_match(session_wd);
    let filter_norm = normalize_path_for_session_search_match(filter);
    if filter_norm.is_empty() {
        return true;
    }

    if session_norm == filter_norm {
        return true;
    }

    let filter_with_sep = format!("{filter_norm}/");
    if session_norm.starts_with(&filter_with_sep) {
        return true;
    }

    // If the user supplied only a project name or path fragment, keep substring
    // matching as a fallback. This preserves the previous loose behavior while
    // making absolute path filters deterministic above.
    !filter_norm.contains('/') && session_norm.contains(&filter_norm)
}

pub fn normalize_path_for_session_search_match(path: &str) -> String {
    path.trim()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

/// Extract a snippet around the first match.
pub fn extract_session_search_snippet(
    text: &str,
    anchor: Option<usize>,
    query: &SessionSearchQueryProfile,
    max_len: usize,
) -> String {
    if let Some(pos) = anchor {
        let focus_len = if !query.normalized.is_empty() {
            query.normalized.len()
        } else {
            query.terms.first().map(|term| term.len()).unwrap_or(0)
        };
        let start = pos.saturating_sub(max_len / 2);
        let end = (pos + focus_len + max_len / 2).min(text.len());

        let start = floor_char_boundary(text, start);
        let end = ceil_char_boundary(text, end);

        let start = text[..start]
            .rfind(char::is_whitespace)
            .map(|p| p + 1)
            .unwrap_or(start);
        let end = text[end..]
            .find(char::is_whitespace)
            .map(|p| end + p)
            .unwrap_or(end);

        let mut snippet = text[start..end].to_string();
        if start > 0 {
            snippet = format!("...{}", snippet);
        }
        if end < text.len() {
            snippet = format!("{}...", snippet);
        }
        snippet
    } else {
        text.chars().take(max_len).collect()
    }
}

fn floor_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut idx = i;
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, i: usize) -> usize {
    if i >= s.len() {
        return s.len();
    }
    let mut idx = i;
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx.min(s.len())
}

#[cfg(test)]
mod session_search_tests {
    use super::*;

    #[test]
    fn query_profile_filters_stop_words_and_requires_actionable_terms() {
        let empty = SessionSearchQueryProfile::new("the and of");
        assert!(!empty.is_actionable());

        let query = SessionSearchQueryProfile::new("AirPods reconnect bluetooth bluetooth");
        assert_eq!(query.terms, vec!["airpods", "reconnect", "bluetooth"]);
        assert_eq!(query.min_term_matches, 2);
        assert!(query.is_actionable());
    }

    #[test]
    fn score_text_match_handles_token_overlap_without_exact_phrase() {
        let query = SessionSearchQueryProfile::new("airpods reconnect bluetooth");
        let score = score_session_search_text_match(
            "Try reconnecting your AirPods after the Bluetooth audio drops.",
            &query,
        )
        .expect("token overlap should match");

        assert!(!score.exact_match);
        assert!(score.matched_terms.contains(&"airpods".to_string()));
        assert!(score.snippet.to_lowercase().contains("airpods"));
    }

    #[test]
    fn raw_and_path_matching_are_case_insensitive() {
        let query = SessionSearchQueryProfile::new("Project Needle");
        assert!(session_search_raw_matches_query(
            b"logs mention project needle here",
            &query
        ));
        assert!(session_search_path_matches_query(
            "/TMP/PROJECT/NEEDLE.json",
            &query
        ));
    }

    #[test]
    fn working_dir_match_is_case_insensitive_and_prefix_based() {
        assert!(session_search_working_dir_matches(
            "/tmp/Project/Subdir",
            "/TMP/project"
        ));
        assert!(session_search_working_dir_matches(
            "/workspace/jcode",
            "jcode"
        ));
        assert!(!session_search_working_dir_matches(
            "/workspace/jcode",
            "/workspace/other"
        ));
    }

    #[test]
    fn snippet_respects_utf8_boundaries() {
        let query = SessionSearchQueryProfile::new("needle");
        let text = "αβγ before needle after δεζ";
        let snippet = extract_session_search_snippet(text, text.find("needle"), &query, 12);
        assert!(snippet.contains("needle"));
    }
}
