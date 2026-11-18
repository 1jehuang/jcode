//! Plain-text summaries and truncation for mobile push payloads.

pub(super) fn cached_regex(
    cache: &'static std::sync::OnceLock<Option<regex::Regex>>,
    pattern: &str,
) -> Option<&'static regex::Regex> {
    cache
        .get_or_init(|| match regex::Regex::new(pattern) {
            Ok(regex) => Some(regex),
            Err(error) => {
                crate::logging::warn(&format!("invalid mobile push summary regex: {error}"));
                None
            }
        })
        .as_ref()
}

/// Strip inline Markdown so a notification reads as plain prose.
pub(super) fn plain_inline(text: &str) -> String {
    static LINK: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    static EMPH: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let link = cached_regex(&LINK, r"!?\[([^\]]*)\]\([^)]*\)");
    let emph = cached_regex(
        &EMPH,
        r"\*\*([^*]+)\*\*|__([^_]+)__|\*([^*\s][^*]*)\*|~~([^~]+)~~",
    );
    let text = match link {
        Some(link) => link.replace_all(text, "$1"),
        None => std::borrow::Cow::Borrowed(text),
    };
    let text = match emph {
        Some(emph) => emph.replace_all(&text, "$1$2$3$4"),
        None => std::borrow::Cow::Borrowed(text.as_ref()),
    };
    text.replace('`', "")
}

/// Turn Markdown into plain lines: drop code blocks, tables, rules and
/// headings markers, list bullets and inline emphasis.
pub(super) fn plain_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.is_empty() {
            if !in_fence && out.last().is_some_and(|l: &String| !l.is_empty()) {
                out.push(String::new());
            }
            continue;
        }
        if line.starts_with('|')
            || line
                .chars()
                .all(|c| matches!(c, '-' | '*' | '_' | '=' | ' '))
        {
            continue;
        }
        // Headings are labels ("Summary"), not content.
        if line.starts_with('#') {
            continue;
        }
        let line = line.trim_start_matches('>').trim();
        let line = strip_list_marker(line);
        let line = plain_inline(line);
        let line = line.trim();
        if !line.is_empty() {
            out.push(line.to_string());
        }
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out
}

pub(super) fn strip_list_marker(line: &str) -> &str {
    for marker in ["- [ ] ", "- [x] ", "- ", "* ", "+ ", "• "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest;
        }
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(rest) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return rest;
        }
    }
    line
}

/// Lead of a final answer: the first paragraph, which by convention carries
/// the conclusion. Bullet items are joined into one sentence-like line.
pub(super) fn summarize_answer(text: &str) -> String {
    let lines = plain_lines(text);
    let mut para = Vec::new();
    for line in &lines {
        if line.is_empty() {
            if !para.is_empty() && para.iter().map(|l: &&String| l.len()).sum::<usize>() >= 40 {
                break;
            }
            continue;
        }
        para.push(line);
    }
    join_sentences(para.iter().map(|l| l.as_str()))
}

pub(super) fn join_sentences<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    let mut out = String::new();
    for part in parts {
        if !out.is_empty() {
            if !out.ends_with(['.', '!', '?', ':', ';']) {
                out.push('.');
            }
            out.push(' ');
        }
        out.push_str(part);
    }
    out
}

/// If the answer ends by asking the user a question, return that question.
pub(super) fn trailing_question(text: &str) -> Option<String> {
    let lines = plain_lines(text);
    let last = lines.iter().rev().find(|l| !l.is_empty())?;
    if !last.trim_end_matches(['*', '_', ')']).ends_with('?') {
        return None;
    }
    // Keep the last sentence, plus the one before if it sets up the question.
    let mut start = 0;
    for (i, c) in last.char_indices() {
        if matches!(c, '.' | '!' | '?') && i + 1 < last.len() - 1 {
            let next = last[i + c.len_utf8()..].chars().next();
            if next.is_some_and(char::is_whitespace) {
                start = i + c.len_utf8();
            }
        }
    }
    let question = last[start..].trim();
    (question.chars().count() >= 8).then(|| question.to_string())
}

/// Errors arrive as `context: cause: cause`. Drop JSON blobs and noise, keep
/// the most specific readable cause.
pub(super) fn summarize_error(text: &str) -> String {
    // A provider failover prompt is machine-readable; say it in plain words.
    if let Some(prompt) = jcode_provider_core::failover::parse_failover_prompt_message(text) {
        return format!(
            "{} is unavailable. Open the session to continue on {}.",
            prompt.from_label, prompt.to_label
        );
    }
    static JSON: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(json) = cached_regex(&JSON, r"\{.*\}") else {
        return flat;
    };
    // Pull a human message out of an embedded provider JSON error if present.
    let message = json.find(&flat).and_then(|m| {
        match serde_json::from_str::<serde_json::Value>(m.as_str()) {
            Ok(value) => Some(value),
            Err(error) => {
                crate::logging::debug(&format!(
                    "mobile push error contains non-JSON braces: {error}"
                ));
                None
            }
        }
        .and_then(|v| {
            v.pointer("/error/message")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
    });
    let stripped = json.replace_all(&flat, "");
    let mut parts: Vec<&str> = stripped
        .split(": ")
        .map(|p| p.trim().trim_end_matches([':', ',']))
        .filter(|p| !p.is_empty())
        .collect();
    parts.dedup();
    let head = parts.join(": ");
    match message {
        Some(m) if !head.contains(m.as_str()) => {
            if head.is_empty() {
                m
            } else {
                format!("{m} ({head})")
            }
        }
        _ => head,
    }
}

/// Wake notifications are agent-facing Markdown. Rewrite the known shapes
/// into one readable line and strip instructions meant for the model.
pub(super) fn summarize_wake(text: &str) -> String {
    if let Some(task) = crate::message::parse_background_task_notification_markdown(text) {
        let label = task
            .display_name
            .clone()
            .unwrap_or_else(|| task.tool_name.clone());
        let status = task.status.trim_start_matches(['✓', '✗', '↻', ' ']).trim();
        let duration = humanize_secs(&task.duration);
        let mut line = format!("{label} {status} in {duration}");
        if status == "failed"
            && task.exit_label.starts_with("exit ")
            && task.exit_label != "exit n/a"
        {
            line.push_str(&format!(" ({})", task.exit_label));
        }
        let detail = task
            .failure_summary
            .as_deref()
            .map(plain_inline)
            .or_else(|| task.preview.as_deref().and_then(last_meaningful_line));
        if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
            line.push_str(": ");
            line.push_str(detail.trim());
        }
        return line;
    }
    if let Some(rest) = text.trim().strip_prefix("**Background task stalled** ") {
        // Incomplete optional wake headers intentionally render missing pieces empty.
        let header = rest.lines().next().unwrap_or("");
        let label = match header.split(" · ").nth(1) {
            Some(label) => plain_inline(label),
            None => String::new(),
        };
        let label = label.split(" (").next().unwrap_or("").trim().to_string();
        let idle = match header
            .split("no output or progress for ")
            .nth(1)
            .and_then(|s| s.split(' ').next())
        {
            Some(idle) => humanize_secs(idle),
            None => String::new(),
        };
        return format!("{label} has been silent for {idle} and may be stuck")
            .trim()
            .to_string();
    }
    let lines: Vec<String> = plain_lines(text)
        .into_iter()
        .filter(|l| !l.is_empty() && !l.contains("action=\"") && !l.starts_with("Full output:"))
        .collect();
    join_sentences(lines.iter().map(String::as_str))
}

pub(super) fn last_meaningful_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .rev()
        .find(|l| l.chars().filter(|c| c.is_alphanumeric()).count() >= 4)
        .map(|l| truncate_chars(l, 120))
}

pub(super) fn humanize_secs(value: &str) -> String {
    let Ok(secs) = value.trim_end_matches('s').parse::<f64>() else {
        return value.to_string();
    };
    let secs = secs.round() as u64;
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => match secs % 60 {
            0 => format!("{}m", secs / 60),
            s => format!("{}m {s}s", secs / 60),
        },
        _ => format!("{}h {}m", secs / 3600, (secs % 3600) / 60),
    }
}

/// Truncate at a word boundary so previews never end mid-word.
pub(super) fn truncate_words(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max.saturating_sub(1)).collect();
    let cut = head
        .rfind(char::is_whitespace)
        .filter(|&i| i >= head.len() * 2 / 3)
        .unwrap_or(head.len());
    let mut out = head[..cut]
        .trim_end_matches([',', ';', ':', ' ', '-'])
        .to_string();
    out.push('…');
    out
}

/// Last `max` chars are most useful for an assistant reply preview? No: the
/// start of a sentence reads better, so keep the head.
pub(super) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
