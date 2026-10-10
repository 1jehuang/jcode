//! Recovery for tool calls that a model emitted as XML/DSML-style markup in
//! plain assistant text instead of as provider-native structured tool calls
//! (issue #1702).
//!
//! OpenAI-compatible and DeepSeek-family endpoints occasionally drift, at long
//! context, into printing their internal tool-call envelope as text:
//!
//! ```text
//! <｜DSML｜function_calls>
//! <｜DSML｜invoke name="bash">
//! <｜DSML｜parameter name="command" string="true">ls</｜DSML｜parameter>
//! </｜DSML｜invoke>
//! </｜DSML｜function_calls>
//! ```
//!
//! Variants seen in the wild drop or mangle the `｜DSML｜` prefix (`</DSML
//! parameter>`), use a bare `invoke`/`parameter` pair, or wrap the calls in
//! `calls`/`function_calls`/`tool_calls`. This module only ever looks at
//! assistant *text*, and callers only invoke it when the response carried no
//! structured tool calls, so structured tool-call arguments are never
//! inspected or rewritten.

use regex::Regex;
use serde_json::{Map, Value};
use std::sync::LazyLock;

/// One tool call recovered from markup.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MarkupToolCall {
    pub(crate) name: String,
    pub(crate) arguments: Value,
}

/// Result of scanning assistant text for tool-call markup.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum MarkupScan {
    /// No tool-call-shaped markup outside code spans.
    None,
    /// Every envelope parsed cleanly. `sanitized_text` is the text with the
    /// envelope removed (prose before and after it is kept).
    Parsed {
        calls: Vec<MarkupToolCall>,
        sanitized_text: String,
    },
    /// The text clearly contains a tool-call envelope, but it could not be
    /// turned into well-formed calls (missing tool name, orphan parameters,
    /// unterminated parameter, ...). Nothing should be executed. `reason`
    /// describes the first problem for diagnostics.
    Unparseable { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Wrapper,
    Invoke,
    Parameter,
}

#[derive(Debug, Clone)]
struct Tag {
    start: usize,
    end: usize,
    closing: bool,
    kind: TagKind,
    attrs: String,
}

static TAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    // Optional namespace-ish prefix: `｜DSML｜`, `|DSML|`, `DSML ` or `antml:`.
    Regex::new(
        r"(?i)<\s*(/)?\s*(?:[｜|]\s*)?(?:DSML\s*(?:[｜|]\s*)?)?(?:antml:)?(function_calls|tool_calls|calls|invoke|parameter)\b([^<>]*)>",
    )
    .expect("valid tool markup tag regex")
});

static ATTR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"([A-Za-z_][\w-]*)\s*=\s*"([^"]*)""#).expect("valid tool markup attr regex")
});

/// Byte ranges covered by fenced code blocks (``` or ~~~). Markup inside them
/// is documentation the model is showing, not a call it is making.
fn fenced_code_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut open: Option<(usize, &str)> = None;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence = if trimmed.starts_with("```") {
            Some("```")
        } else if trimmed.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        match (open, fence) {
            (None, Some(f)) => open = Some((offset, f)),
            (Some((start, f)), Some(g)) if f == g => {
                ranges.push((start, offset + line.len()));
                open = None;
            }
            _ => {}
        }
        offset += line.len();
    }
    if let Some((start, _)) = open {
        ranges.push((start, text.len()));
    }
    ranges
}

fn collect_tags(text: &str) -> Vec<Tag> {
    let fences = fenced_code_ranges(text);
    TAG_RE
        .captures_iter(text)
        .filter_map(|caps| {
            let whole = caps.get(0)?;
            let start = whole.start();
            if fences.iter().any(|&(s, e)| start >= s && start < e) {
                return None;
            }
            // `<invoke>` quoted inline with backticks is prose.
            if text[..start].ends_with('`') {
                return None;
            }
            let kind = match caps.get(2)?.as_str().to_ascii_lowercase().as_str() {
                "invoke" => TagKind::Invoke,
                "parameter" => TagKind::Parameter,
                _ => TagKind::Wrapper,
            };
            Some(Tag {
                start,
                end: whole.end(),
                closing: caps.get(1).is_some(),
                kind,
                attrs: caps
                    .get(3)
                    .map(|m| m.as_str().to_string())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn attr(attrs: &str, key: &str) -> Option<String> {
    ATTR_RE
        .captures_iter(attrs)
        .find(|caps| caps[1].eq_ignore_ascii_case(key))
        .map(|caps| caps[2].to_string())
}

/// Convert a raw parameter body into a JSON value.
///
/// DSML marks raw strings with `string="true"` and JSON literals with
/// `string="false"`. Without the hint, scalars and containers that parse as
/// JSON (numbers, booleans, objects, arrays) keep their JSON type and
/// everything else is a string. String values are kept byte-for-byte.
fn parameter_value(raw: &str, attrs: &str) -> Value {
    let hint = attr(attrs, "string").map(|v| v.to_ascii_lowercase());
    if hint.as_deref() == Some("true") {
        return Value::String(raw.to_string());
    }
    let trimmed = raw.trim();
    match serde_json::from_str::<Value>(trimmed) {
        Ok(Value::String(_)) if hint.as_deref() != Some("false") => Value::String(raw.to_string()),
        Ok(value) if !trimmed.is_empty() => value,
        _ => Value::String(raw.to_string()),
    }
}

/// Scan `text` for tool-call markup and try to turn it into tool calls.
pub(crate) fn scan_tool_call_markup(text: &str) -> MarkupScan {
    let tags = collect_tags(text);
    let relevant = tags
        .iter()
        .filter(|tag| tag.kind != TagKind::Wrapper)
        .count();
    // A single stray tag is far more likely to be prose than a call.
    if relevant < 2 {
        return MarkupScan::None;
    }

    let mut calls = Vec::new();
    let mut problem: Option<String> = None;
    let mut current: Option<(Option<String>, Map<String, Value>)> = None;
    let mut i = 0usize;
    while i < tags.len() {
        let tag = &tags[i];
        match (tag.kind, tag.closing) {
            (TagKind::Wrapper, _) => {}
            (TagKind::Invoke, false) => {
                if current.is_some() && problem.is_none() {
                    problem = Some("nested or unterminated invoke".to_string());
                }
                current = Some((
                    attr(&tag.attrs, "name").filter(|n| !n.trim().is_empty()),
                    Map::new(),
                ));
            }
            (TagKind::Invoke, true) => match current.take() {
                Some((Some(name), arguments)) => calls.push(MarkupToolCall {
                    name: name.trim().to_string(),
                    arguments: Value::Object(arguments),
                }),
                Some((None, _)) => {
                    problem.get_or_insert_with(|| "invoke without a tool name".to_string());
                }
                None => {
                    problem.get_or_insert_with(|| {
                        "closing invoke without a matching opening invoke".to_string()
                    });
                }
            },
            (TagKind::Parameter, false) => {
                // Parameter bodies are raw: the body ends at the next closing
                // parameter tag regardless of any markup inside it.
                let Some(close_idx) = (i + 1..tags.len())
                    .find(|&j| tags[j].kind == TagKind::Parameter && tags[j].closing)
                else {
                    problem.get_or_insert_with(|| "unterminated parameter".to_string());
                    break;
                };
                let body = &text[tag.end..tags[close_idx].start];
                match (&mut current, attr(&tag.attrs, "name")) {
                    (Some((_, arguments)), Some(name)) if !name.trim().is_empty() => {
                        arguments
                            .insert(name.trim().to_string(), parameter_value(body, &tag.attrs));
                    }
                    (Some(_), _) => {
                        problem.get_or_insert_with(|| "parameter without a name".to_string());
                    }
                    (None, _) => {
                        problem.get_or_insert_with(|| {
                            "parameter outside of an invoke block".to_string()
                        });
                    }
                }
                i = close_idx;
            }
            (TagKind::Parameter, true) => {
                problem.get_or_insert_with(|| "closing parameter without an opening".to_string());
            }
        }
        i += 1;
    }
    if current.is_some() {
        problem.get_or_insert_with(|| "unterminated invoke".to_string());
    }
    if let Some(reason) = problem {
        return MarkupScan::Unparseable { reason };
    }
    if calls.is_empty() {
        return MarkupScan::Unparseable {
            reason: "no complete invoke block".to_string(),
        };
    }

    let first = tags.first().map(|t| t.start).unwrap_or(0);
    let last = tags.last().map(|t| t.end).unwrap_or(text.len());
    let prefix = text[..first].trim_end();
    let suffix = text[last..].trim();
    let mut sanitized_text = prefix.to_string();
    if !suffix.is_empty() {
        if !sanitized_text.is_empty() {
            sanitized_text.push('\n');
        }
        sanitized_text.push_str(suffix);
    }
    MarkupScan::Parsed {
        calls,
        sanitized_text,
    }
}

/// Model-facing reminder injected when an envelope could not be parsed.
pub(crate) fn unparsed_markup_reminder(reason: &str) -> String {
    format!(
        "<system-reminder>Your previous response contained a tool call written as plain-text markup (invoke/parameter envelope) instead of a native structured tool call. It could not be parsed ({reason}), so nothing was executed. Re-issue the intended tool call now using the native tool-calling interface, with the tool name and a complete JSON object of arguments. Do not print tool-call markup as text and do not repeat completed work.</system-reminder>"
    )
}

/// User-visible diagnostic for the same situation.
pub(crate) fn unparsed_markup_notice(reason: &str) -> String {
    format!(
        "[tool-call recovery] The model printed a tool call as text markup that could not be parsed ({reason}). Nothing was executed. Asking the model to re-issue it as a native tool call."
    )
}

#[cfg(test)]
#[path = "tool_markup_recovery_tests.rs"]
mod tests;
