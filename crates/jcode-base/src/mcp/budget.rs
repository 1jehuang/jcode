//! Adaptive token budgets for MCP results.
//!
//! MCP servers return anything from a one-line acknowledgement to megabytes
//! of JSON. A fixed character cap either wastes context on small results or
//! chops large ones mid-structure. Instead, results under budget pass through
//! untouched, and larger ones are squeezed just enough to fit:
//!
//! - JSON keeps its shape. Long strings are shortened and long arrays keep
//!   their leading items, using the loosest caps that fit the budget, so a
//!   result 10% over loses a little from each long field while a result 20x
//!   over keeps a structural sample.
//! - Text keeps its start and end on line boundaries, where headers,
//!   summaries and errors usually live.
//!
//! A trimmed result ends with a note giving both sizes (parsed by Desktop for
//! its token badge) and how to get the full result.

use jcode_core::util::estimate_tokens;
use serde_json::Value;

/// Default budget for one MCP tool result.
pub const MCP_RESULT_TOKEN_BUDGET: usize = 8_000;

/// Prefix of the note appended to trimmed results. Desktop parses
/// `"{PREFIX}{original} to {kept} tokens"` for its token badge.
pub const TRIMMED_NOTE_PREFIX: &str = "[MCP result trimmed from ";

/// Shortest string a JSON squeeze keeps before marking the cut.
const MIN_STRING_CHARS: usize = 48;
/// Fewest array items a JSON squeeze keeps before marking the rest.
const MIN_ARRAY_ITEMS: usize = 2;

/// Fit `text` into `budget_tokens`, returning it unchanged when it already fits.
pub fn fit_to_budget(text: &str, budget_tokens: usize) -> String {
    let original = estimate_tokens(text);
    if original <= budget_tokens {
        return text.to_string();
    }
    let fitted = match serde_json::from_str::<Value>(text.trim()) {
        Ok(value) if value.is_object() || value.is_array() => {
            fit_json(&value, budget_tokens).unwrap_or_else(|| fit_text(text, budget_tokens))
        }
        _ => fit_text(text, budget_tokens),
    };
    format!(
        "{fitted}\n\n{TRIMMED_NOTE_PREFIX}{original} to {} tokens to fit the context budget. \
         Pass accept_large_output: true for the full result, or narrow the request.]",
        estimate_tokens(&fitted)
    )
}

/// The original and kept token counts of a trimmed result, if it was trimmed.
pub fn parse_trimmed_note(output: &str) -> Option<(usize, usize)> {
    let rest = &output[output.rfind(TRIMMED_NOTE_PREFIX)? + TRIMMED_NOTE_PREFIX.len()..];
    let (original, rest) = rest.split_once(" to ")?;
    let kept = rest.split_once(' ')?.0;
    Some((original.parse().ok()?, kept.parse().ok()?))
}

/// Shrink JSON with the loosest string and array caps that fit. Strings are
/// tightened first, since long text fields are usually the bulk and items are
/// usually the information; arrays are capped only when strings alone cannot
/// get under budget.
fn fit_json(value: &Value, budget_tokens: usize) -> Option<String> {
    let fits = |strings: usize, items: usize| {
        let out = render(value, strings, items);
        (estimate_tokens(&out) <= budget_tokens).then_some(out)
    };
    let longest = longest_string(value).max(MIN_STRING_CHARS);
    let widest = widest_array(value).max(MIN_ARRAY_ITEMS);

    if let Some(out) = fits(MIN_STRING_CHARS, widest) {
        // Arrays can stay whole: find the longest strings that still fit.
        let cap = max_fitting(MIN_STRING_CHARS, longest, |cap| fits(cap, widest).is_some());
        return fits(cap, widest).or(Some(out));
    }
    // Strings at their floor are not enough: cap arrays too.
    let items = max_fitting(MIN_ARRAY_ITEMS, widest, |items| {
        fits(MIN_STRING_CHARS, items).is_some()
    });
    fits(MIN_STRING_CHARS, items)
}

/// Largest `n` in `lo..=hi` for which `ok(n)` holds, assuming `ok` is
/// monotone (true up to some point). Returns `lo` if nothing fits.
fn max_fitting(lo: usize, hi: usize, ok: impl Fn(usize) -> bool) -> usize {
    let (mut lo, mut hi) = (lo, hi.max(lo));
    if ok(hi) {
        return hi;
    }
    while lo + 1 < hi {
        let mid = lo + (hi - lo) / 2;
        if ok(mid) { lo = mid } else { hi = mid }
    }
    lo
}

fn render(value: &Value, string_cap: usize, item_cap: usize) -> String {
    serde_json::to_string(&squeeze(value, string_cap, item_cap)).unwrap_or_default()
}

fn squeeze(value: &Value, string_cap: usize, item_cap: usize) -> Value {
    match value {
        Value::String(text) => {
            let count = text.chars().count();
            if count <= string_cap {
                return value.clone();
            }
            let kept: String = text.chars().take(string_cap).collect();
            Value::String(format!("{kept}… [+{} chars]", count - string_cap))
        }
        Value::Array(items) => {
            let mut out: Vec<Value> = items
                .iter()
                .take(item_cap)
                .map(|item| squeeze(item, string_cap, item_cap))
                .collect();
            if items.len() > item_cap {
                out.push(Value::String(format!(
                    "… {} more items",
                    items.len() - item_cap
                )));
            }
            Value::Array(out)
        }
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, item)| (key.clone(), squeeze(item, string_cap, item_cap)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn longest_string(value: &Value) -> usize {
    match value {
        Value::String(text) => text.chars().count(),
        Value::Array(items) => items.iter().map(longest_string).max().unwrap_or(0),
        Value::Object(map) => map.values().map(longest_string).max().unwrap_or(0),
        _ => 0,
    }
}

fn widest_array(value: &Value) -> usize {
    match value {
        Value::Array(items) => items
            .iter()
            .map(widest_array)
            .max()
            .unwrap_or(0)
            .max(items.len()),
        Value::Object(map) => map.values().map(widest_array).max().unwrap_or(0),
        _ => 0,
    }
}

/// Keep roughly the first three quarters and last quarter of the budget,
/// cut on line boundaries when lines are short enough to allow it.
fn fit_text(text: &str, budget_tokens: usize) -> String {
    let budget_bytes = budget_tokens * (text.len() / estimate_tokens(text).max(1)).max(1);
    let head_bytes = budget_bytes * 3 / 4;
    let tail_bytes = budget_bytes.saturating_sub(head_bytes);

    let mut head_end = floor_char_boundary(text, head_bytes);
    if let Some(newline) = text[..head_end].rfind('\n')
        && newline >= head_end / 2
    {
        head_end = newline;
    }
    let mut tail_start = ceil_char_boundary(text, text.len().saturating_sub(tail_bytes));
    if let Some(newline) = text[tail_start..].find('\n')
        && newline <= (text.len() - tail_start) / 2
    {
        tail_start += newline + 1;
    }
    if tail_start <= head_end {
        return text[..head_end].to_string();
    }
    let omitted = estimate_tokens(&text[head_end..tail_start]);
    format!(
        "{}\n[… ~{omitted} tokens omitted …]\n{}",
        &text[..head_end],
        &text[tail_start..]
    )
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_char_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn results_under_budget_pass_through_untouched() {
        let text = "small result";
        assert_eq!(fit_to_budget(text, 100), text);
        assert!(parse_trimmed_note(text).is_none());
    }

    #[test]
    fn slightly_large_json_only_shortens_long_strings() {
        let value = json!({
            "items": (0..5).map(|n| json!({"id": n, "body": "x".repeat(2_000)})).collect::<Vec<_>>()
        });
        let text = value.to_string();
        let budget = estimate_tokens(&text) * 3 / 4;
        let out = fit_to_budget(&text, budget);
        let (original, kept) = parse_trimmed_note(&out).unwrap();
        assert_eq!(original, estimate_tokens(&text));
        assert!(kept <= budget, "{kept} > {budget}");
        let body = out.split("\n\n[MCP result trimmed").next().unwrap();
        let fitted: Value = serde_json::from_str(body).unwrap();
        // Every item survives; only the long strings were shortened.
        assert_eq!(fitted["items"].as_array().unwrap().len(), 5);
        assert!(fitted["items"][0]["body"].as_str().unwrap().contains("[+"));
        // The squeeze is adaptive: far more than the floor was kept.
        assert!(fitted["items"][0]["body"].as_str().unwrap().len() > 1_000);
    }

    #[test]
    fn very_large_json_also_samples_arrays() {
        let value: Vec<Value> = (0..5_000)
            .map(|n| json!({"id": n, "name": format!("row {n}")}))
            .collect();
        let text = serde_json::to_string(&value).unwrap();
        let out = fit_to_budget(&text, 1_000);
        let (_, kept) = parse_trimmed_note(&out).unwrap();
        assert!(kept <= 1_000);
        let body = out.split("\n\n[MCP result trimmed").next().unwrap();
        let fitted: Vec<Value> = serde_json::from_str(body).unwrap();
        assert!(fitted.len() > MIN_ARRAY_ITEMS, "keeps as many rows as fit");
        assert!(
            fitted
                .last()
                .unwrap()
                .as_str()
                .unwrap()
                .contains("more items")
        );
        assert_eq!(fitted[0]["id"], 0);
    }

    #[test]
    fn text_keeps_its_start_and_end() {
        let text: String = (0..2_000).map(|n| format!("line {n}\n")).collect();
        let out = fit_to_budget(&text, 500);
        assert!(out.starts_with("line 0\n"));
        assert!(out.contains("line 1999"));
        assert!(out.contains("tokens omitted"));
        let (_, kept) = parse_trimmed_note(&out).unwrap();
        assert!(kept <= 520, "{kept}");
    }

    #[test]
    fn multibyte_text_is_cut_on_char_boundaries() {
        let text = "é".repeat(20_000);
        let out = fit_to_budget(&text, 300);
        assert!(out.contains("tokens omitted"));
    }
}
