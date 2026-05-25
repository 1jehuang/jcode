use super::tool_output_looks_failed;

/// Parse batch result content to determine per-sub-call success/error.
/// Returns a Vec<bool> where `true` means that sub-call errored.
/// The batch output format is:
///   --- [1] tool_name ---
///   <output or Error: ...>
///   --- [2] tool_name ---
///   ...
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BatchSubResult {
    pub errored: bool,
    pub content: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BatchCompletionCounts {
    pub succeeded: usize,
    pub failed: usize,
}

impl BatchCompletionCounts {
    pub(crate) fn total(self) -> usize {
        self.succeeded + self.failed
    }
}

pub(crate) fn is_batch_section_header(line: &str) -> bool {
    line.starts_with("--- [") && line.ends_with(" ---")
}

pub(crate) fn batch_section_index(line: &str) -> Option<usize> {
    if !is_batch_section_header(line) {
        return None;
    }
    let rest = line.strip_prefix("--- [")?;
    let (index, _) = rest.split_once(']')?;
    index.parse::<usize>().ok()
}

pub(crate) fn is_batch_footer_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("Completed: ") else {
        return false;
    };
    let Some((successes, rest)) = rest.split_once(" succeeded, ") else {
        return false;
    };
    let Some(failures) = rest.strip_suffix(" failed") else {
        return false;
    };

    !successes.is_empty()
        && !failures.is_empty()
        && successes.chars().all(|ch| ch.is_ascii_digit())
        && failures.chars().all(|ch| ch.is_ascii_digit())
}

pub(crate) fn parse_batch_completion_counts(content: &str) -> Option<BatchCompletionCounts> {
    for line in content.lines().rev() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Completed: ") else {
            continue;
        };
        let Some((successes, rest)) = rest.split_once(" succeeded, ") else {
            continue;
        };
        let Some(failures) = rest.strip_suffix(" failed") else {
            continue;
        };
        let Ok(succeeded) = successes.parse::<usize>() else {
            continue;
        };
        let Ok(failed) = failures.parse::<usize>() else {
            continue;
        };
        return Some(BatchCompletionCounts { succeeded, failed });
    }
    None
}

pub(crate) fn finalize_batch_section(raw: &str) -> BatchSubResult {
    let mut content = raw.trim_end_matches(['\n', '\r']).to_string();
    if let Some((body, footer)) = content.rsplit_once("\n\n") {
        if is_batch_footer_line(footer.trim()) {
            content = body.trim_end_matches(['\n', '\r']).to_string();
        }
    } else if is_batch_footer_line(content.trim()) {
        content.clear();
    }

    let errored = tool_output_looks_failed(&content);

    BatchSubResult { errored, content }
}

#[cfg(test)]
pub(crate) fn parse_batch_sub_outputs(content: &str) -> Vec<BatchSubResult> {
    parse_batch_sub_outputs_by_index(content)
        .into_values()
        .collect()
}

pub(crate) fn parse_batch_sub_outputs_by_index(
    content: &str,
) -> std::collections::BTreeMap<usize, BatchSubResult> {
    let mut results = std::collections::BTreeMap::new();
    let mut current_index: Option<usize> = None;
    let mut current_content_start: Option<usize> = None;
    let mut current_pos = 0usize;

    while current_pos < content.len() {
        let line_start = current_pos;
        let rest = &content[current_pos..];
        let (line, next_pos) = if let Some(rel_end) = rest.find('\n') {
            let end = current_pos + rel_end + 1;
            (&content[current_pos..end], end)
        } else {
            (&content[current_pos..], content.len())
        };
        current_pos = next_pos;
        let trimmed = line.trim_end_matches(['\n', '\r']);

        if let Some(index) = batch_section_index(trimmed) {
            if let (Some(prev_index), Some(start)) = (current_index, current_content_start) {
                results.insert(
                    prev_index,
                    finalize_batch_section(&content[start..line_start]),
                );
            }
            current_index = Some(index);
            current_content_start = Some(current_pos);
        }
    }

    if let (Some(index), Some(start)) = (current_index, current_content_start) {
        results.insert(index, finalize_batch_section(&content[start..]));
    }

    results
}

/// Normalize a batch sub-call object to the effective parameters payload.
/// Supports both canonical shape ({"tool": "...", "parameters": {...}})
/// and recovered flat shape ({"tool": "...", "file_path": "...", ...}).
pub(crate) fn batch_subcall_params(call: &serde_json::Value) -> serde_json::Value {
    if let Some(params) = call.get("parameters") {
        return params.clone();
    }

    if let Some(obj) = call.as_object() {
        let mut flat = serde_json::Map::new();
        for (k, v) in obj {
            if k != "tool" && k != "name" && k != "intent" {
                flat.insert(k.clone(), v.clone());
            }
        }
        return serde_json::Value::Object(flat);
    }

    serde_json::Value::Object(serde_json::Map::new())
}
