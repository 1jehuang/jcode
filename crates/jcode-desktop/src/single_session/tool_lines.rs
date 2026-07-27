use super::*;

pub(crate) fn append_tool_lines(
    lines: &mut Vec<SingleSessionStyledLine>,
    content: &str,
    active: bool,
    active_input: Option<&str>,
    tool_run: Option<&SingleSessionToolRun>,
) {
    if content.is_empty() {
        return;
    }
    let mut raw_lines = content.lines();
    let Some(header) = raw_lines.next() else {
        return;
    };
    let header_is_expanded = header.trim_start().starts_with('▾');
    if !header.trim_start().starts_with(['▾', '▸']) {
        for line in std::iter::once(header).chain(raw_lines) {
            if !line.trim().is_empty() {
                lines.push(styled_line(
                    format!("  {}", line.trim()),
                    SingleSessionLineStyle::Tool,
                ));
            }
        }
        return;
    }
    let header = parse_tool_header(header);
    let tool_state = tool_run
        .map(|run| run.state)
        .or_else(|| {
            header
                .state
                .as_deref()
                .map(SingleSessionToolVisualState::from_tool_state_text)
        })
        .unwrap_or(SingleSessionToolVisualState::Unknown);
    let expanded = active || header_is_expanded;
    let base_metadata = SingleSessionToolLineMetadata {
        call_id: tool_run
            .map(|run| run.call_id.clone())
            .unwrap_or_else(|| fallback_tool_line_call_id(&header)),
        name: tool_run
            .map(|run| run.name.clone())
            .unwrap_or_else(|| header.name.clone()),
        state: tool_state,
        kind: SingleSessionToolLineKind::Header,
        active: active && tool_state.is_active(),
        expanded,
        stdin_prompt: tool_run.and_then(|run| run.stdin_prompt.clone()),
    };
    let mut metadata_lines = Vec::new();
    let mut widget_lines = Vec::new();
    for line in raw_lines {
        if let Some(raw_input) = line.strip_prefix("  input: ") {
            metadata_lines.extend(formatted_tool_input_lines(&header.name, raw_input));
        } else if !line.trim().is_empty() {
            widget_lines.push(compact_tool_widget_text(line.trim(), 112));
        }
    }
    if let Some(raw_input) = active_input.filter(|input| !input.is_empty()) {
        metadata_lines.extend(formatted_tool_input_lines(&header.name, raw_input));
    }
    if metadata_lines.is_empty()
        && let Some(input_preview) = tool_run.and_then(|run| run.input_preview.as_deref())
    {
        metadata_lines.push(input_preview.to_string());
    }
    if let Some(stdin_prompt) = &base_metadata.stdin_prompt {
        metadata_lines.push(format!("input needed: {stdin_prompt}"));
    }

    lines.push(
        styled_line(
            format_tool_header_line_with_metadata(&header, &metadata_lines),
            SingleSessionLineStyle::Tool,
        )
        .with_tool_metadata(base_metadata.clone()),
    );

    if active
        && widget_lines.is_empty()
        && matches!(header.state.as_deref(), Some("preparing") | Some("running"))
    {
        widget_lines.push("waiting for tool output…".to_string());
    }

    if expanded && !widget_lines.is_empty() {
        append_tool_content_widget(lines, &widget_lines, &base_metadata);
    }
}

pub(crate) fn fallback_tool_line_call_id(header: &ToolHeader) -> String {
    format!(
        "legacy-tool:{}:{}:{}",
        header.name,
        header.state.as_deref().unwrap_or("unknown"),
        header.summary.as_deref().unwrap_or_default()
    )
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ToolHeader {
    name: String,
    state: Option<String>,
    summary: Option<String>,
}

pub(crate) fn parse_tool_header(line: &str) -> ToolHeader {
    let line = line.trim().trim_start_matches(['▾', '▸']).trim();
    let mut parts = line.splitn(2, char::is_whitespace);
    let name = parts
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("tool");
    let rest = parts.next().unwrap_or_default().trim();
    if rest.is_empty() {
        return ToolHeader {
            name: name.to_string(),
            state: None,
            summary: None,
        };
    }

    let (state, summary) = rest
        .split_once(':')
        .map(|(state, summary)| (state.trim(), Some(summary.trim())))
        .unwrap_or((rest, None));

    ToolHeader {
        name: name.to_string(),
        state: Some(state.to_string()).filter(|state| !state.is_empty()),
        summary: summary
            .filter(|summary| !summary.is_empty())
            .map(|summary| compact_tool_text(summary, 116)),
    }
}

#[cfg(test)]
pub(crate) fn format_tool_header_line(header: &ToolHeader) -> String {
    format_tool_header_line_with_metadata(header, &[])
}

pub(crate) fn format_tool_header_line_with_metadata(
    header: &ToolHeader,
    metadata_lines: &[String],
) -> String {
    let icon = match header.state.as_deref() {
        Some("done") => "✓",
        Some("failed") => "✕",
        Some("running") => "●",
        Some("preparing") => "○",
        _ => "•",
    };
    let mut line = match (&header.state, &header.summary) {
        (Some(state), Some(summary)) => format!("  {icon} {} · {state} · {summary}", header.name),
        (Some(state), None) => format!("  {icon} {} · {state}", header.name),
        (None, Some(summary)) => format!("  {icon} {} · {summary}", header.name),
        (None, None) => format!("  {icon} {}", header.name),
    };

    if let Some(metadata) = compact_tool_metadata(metadata_lines) {
        line.push_str(" · ");
        line.push_str(&metadata);
    }
    line
}

pub(crate) fn compact_tool_metadata(metadata_lines: &[String]) -> Option<String> {
    let metadata = metadata_lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    (!metadata.is_empty()).then(|| compact_tool_text(&metadata, 116))
}

pub(crate) fn append_tool_content_widget(
    lines: &mut Vec<SingleSessionStyledLine>,
    content_lines: &[String],
    base_metadata: &SingleSessionToolLineMetadata,
) {
    const MAX_WIDGET_LINES: usize = 12;
    for line in content_lines.iter().take(MAX_WIDGET_LINES) {
        lines.push(tool_detail_styled_line(line, base_metadata));
    }
    if content_lines.len() > MAX_WIDGET_LINES {
        lines.push(tool_detail_styled_line(
            &format!("… {} more lines", content_lines.len() - MAX_WIDGET_LINES),
            base_metadata,
        ));
    }
}

pub(crate) fn tool_detail_styled_line(
    line: &str,
    base_metadata: &SingleSessionToolLineMetadata,
) -> SingleSessionStyledLine {
    let mut metadata = base_metadata.clone();
    metadata.kind = SingleSessionToolLineKind::Detail;
    styled_line(
        format!("    {}", compact_tool_widget_text(line, 118)),
        SingleSessionLineStyle::Tool,
    )
    .with_tool_metadata(metadata)
}

pub(crate) fn compact_tool_widget_text(text: &str, max_chars: usize) -> String {
    let text = text.trim().replace('\t', "    ");
    if text.chars().count() > max_chars {
        format!(
            "{}…",
            text.chars()
                .take(max_chars.saturating_sub(1))
                .collect::<String>()
        )
    } else {
        text
    }
}

pub(crate) fn append_tool_group_summary(
    lines: &mut Vec<SingleSessionStyledLine>,
    tool_messages: &[SingleSessionMessage],
) {
    const TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT: usize = 6;

    if tool_messages.is_empty() {
        return;
    }

    let mut names: Vec<String> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut approx_tokens = 0usize;

    for message in tool_messages {
        // This is only a collapsed-card estimate. Counting Unicode scalar
        // values scans every byte of large tool outputs and made first content
        // frames visibly stall for transcripts with huge tool groups. Use the
        // byte length's O(1) metadata instead; it is a good enough token proxy
        // for a summary that is intentionally approximate.
        approx_tokens += message.content().len().div_ceil(4);
        let name = tool_summary_name(message.content());
        if let Some(index) = names.iter().position(|existing| existing == &name) {
            counts[index] += 1;
        } else {
            names.push(name);
            counts.push(1);
        }
    }

    let total_distinct_tools = names.len();
    let mut summary_hasher = DefaultHasher::new();
    names.hash(&mut summary_hasher);
    counts.hash(&mut summary_hasher);
    approx_tokens.hash(&mut summary_hasher);
    let summary_hash = summary_hasher.finish();

    let mut fragments = names
        .into_iter()
        .zip(counts)
        .take(TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT)
        .map(|(name, count)| format!("{count} {name}"))
        .collect::<Vec<_>>();
    if total_distinct_tools > TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT {
        fragments.push(format!(
            "{} more kinds",
            total_distinct_tools - TOOL_GROUP_SUMMARY_VISIBLE_FRAGMENT_LIMIT
        ));
    }
    let fragments = fragments.join(", ");
    let token_fragment = format_approx_tokens(approx_tokens);
    let line = format!("  ▸ tools: {fragments} · ~{token_fragment} tokens");
    lines.push(
        styled_line(line.clone(), SingleSessionLineStyle::Tool).with_tool_metadata(
            SingleSessionToolLineMetadata {
                call_id: format!("tool-group:{summary_hash:016x}"),
                name: "tools".to_string(),
                state: SingleSessionToolVisualState::Group,
                kind: SingleSessionToolLineKind::GroupSummary,
                active: false,
                expanded: false,
                stdin_prompt: None,
            },
        ),
    );
}

pub(crate) fn tool_summary_name(content: &str) -> String {
    content
        .lines()
        .next()
        .unwrap_or("tool")
        .trim_start_matches(['▾', '▸'])
        .split_whitespace()
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("tool")
        .to_string()
}

pub(crate) fn format_approx_tokens(tokens: usize) -> String {
    if tokens >= 10_000 {
        format!("{}k", ((tokens + 500) / 1000))
    } else if tokens >= 1_000 {
        let tenths = (tokens + 50) / 100;
        format!("{}.{}k", tenths / 10, tenths % 10)
    } else {
        tokens.to_string()
    }
}

pub(crate) fn formatted_tool_input_lines(tool_name: &str, raw_input: &str) -> Vec<String> {
    const MAX_INPUT_LINES: usize = 6;
    let raw_input = raw_input.trim();
    if raw_input.is_empty() {
        return vec!["input: <empty>".to_string()];
    }

    if !looks_like_json_value(raw_input) {
        return vec![format!("input: {}", compact_tool_text(raw_input, 132))];
    }

    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw_input) else {
        // Tool arguments arrive as raw JSON deltas, so nothing parses until the
        // final chunk. Every tool states an `intent` up front, so scrape it out
        // of the partial JSON: a plain-English headline beats both a truncated
        // JSON fragment and an empty card while the call streams in.
        if let Some(intent) = partial_json_string_field(raw_input, "intent") {
            return vec![intent];
        }
        return vec![format!("input: {}", compact_tool_text(raw_input, 132))];
    };

    let serde_json::Value::Object(map) = value else {
        return vec![format!(
            "input: {}",
            compact_tool_json_value("input", &value)
        )];
    };

    if map.is_empty() {
        return vec!["input: {}".to_string()];
    }

    if let Some(lines) = formatted_tool_input_summary(tool_name, &map) {
        return lines;
    }

    let mut keys = map.keys().cloned().collect::<Vec<_>>();
    keys.sort_by(|left, right| {
        tool_input_key_priority(left)
            .cmp(&tool_input_key_priority(right))
            .then_with(|| left.cmp(right))
    });

    let total = keys.len();
    let mut rendered = keys
        .into_iter()
        .take(MAX_INPUT_LINES)
        .filter_map(|key| {
            map.get(&key)
                .map(|value| format!("{key}: {}", compact_tool_json_value(&key, value)))
        })
        .collect::<Vec<_>>();
    if total > MAX_INPUT_LINES {
        rendered.push(format!("… {} more", total - MAX_INPUT_LINES));
    }
    rendered
}

/// Extract a top-level JSON string field from possibly-incomplete JSON.
///
/// Tool arguments stream in as raw JSON deltas, so `serde_json` cannot parse
/// them until the final chunk. This scans for `"<field>": "..."` at depth 1 and
/// tolerates a truncated closing quote so the value can be shown mid-stream.
pub(crate) fn partial_json_string_field(raw_input: &str, field: &str) -> Option<String> {
    let bytes = raw_input.as_bytes();
    let needle = format!("\"{field}\"");
    let mut search_from = 0usize;
    while let Some(offset) = raw_input[search_from..].find(&needle) {
        let key_start = search_from + offset;
        search_from = key_start + needle.len();
        if !json_key_is_at_top_level(raw_input, key_start) {
            continue;
        }
        let mut cursor = search_from;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b':' {
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'"' {
            continue;
        }
        cursor += 1;
        let mut value = String::new();
        let mut escaped = false;
        while cursor < bytes.len() {
            let ch = bytes[cursor] as char;
            if escaped {
                value.push(match ch {
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    other => other,
                });
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            } else {
                // Push from the source so multi-byte characters stay intact.
                let rest = &raw_input[cursor..];
                let next = rest.chars().next()?;
                value.push(next);
                cursor += next.len_utf8();
                continue;
            }
            cursor += 1;
        }
        let value = value.trim();
        if value.is_empty() {
            return None;
        }
        return Some(compact_tool_text(value, 132));
    }
    None
}

/// True when the JSON key starting at `key_start` sits directly in the root
/// object, so nested objects (for example `batch` sub-calls) do not hijack the
/// headline.
fn json_key_is_at_top_level(raw_input: &str, key_start: usize) -> bool {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in raw_input.char_indices() {
        if index >= key_start {
            break;
        }
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth == 1 && !in_string
}

pub(crate) fn looks_like_json_value(text: &str) -> bool {
    matches!(
        text.as_bytes().first().copied(),
        Some(b'{' | b'[' | b'"' | b'-' | b'0'..=b'9' | b't' | b'f' | b'n')
    )
}

pub(crate) fn formatted_tool_input_summary(
    tool_name: &str,
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<Vec<String>> {
    let string_value = |key: &str| map.get(key).and_then(serde_json::Value::as_str);
    let bool_value = |key: &str| map.get(key).and_then(serde_json::Value::as_bool);
    let mut lines = Vec::new();

    match tool_name {
        "bash" => {
            if let Some(command) = string_value("command") {
                lines.push(format!("$ {}", compact_tool_text(command, 132)));
            }
        }
        "read" => {
            if let Some(path) = string_value("file_path") {
                lines.push(format!("read {}", compact_tool_text(path, 132)));
            }
        }
        "write" | "edit" | "multiedit" => {
            if let Some(path) = string_value("file_path") {
                let mut summary = compact_tool_text(path, 132);
                if tool_name == "multiedit"
                    && let Some(count) = map
                        .get("edits")
                        .and_then(serde_json::Value::as_array)
                        .map(Vec::len)
                {
                    summary.push_str(&format!(" ({count} edits)"));
                }
                lines.push(summary);
            }
        }
        "glob" => {
            if let Some(pattern) = string_value("pattern") {
                lines.push(format!("'{}'", compact_tool_text(pattern, 96)));
            }
        }
        "agentgrep" | "grep" => {
            let query = string_value("query").or_else(|| string_value("pattern"));
            if tool_name == "agentgrep" {
                let mode = string_value("mode").unwrap_or("grep");
                if let Some(query) = query.filter(|query| !query.trim().is_empty()) {
                    lines.push(format!("{mode} '{}'", compact_tool_text(query, 72)));
                } else {
                    lines.push(mode.to_string());
                }
            } else if let Some(query) = query {
                lines.push(format!("'{}'", compact_tool_text(query, 72)));
            }
            if let Some(path) = string_value("path") {
                lines.push(format!("in {}", compact_tool_text(path, 132)));
            }
        }
        "webfetch" | "websearch" => {
            if let Some(query) = string_value("query").or_else(|| string_value("url")) {
                lines.push(compact_tool_text(query, 132));
            }
        }
        "browser" => {
            if let Some(action) = string_value("action") {
                let target = string_value("url")
                    .or_else(|| string_value("selector"))
                    .or_else(|| string_value("text"));
                lines.push(match target {
                    Some(target) => format!("{action} {}", compact_tool_text(target, 112)),
                    None => action.to_string(),
                });
            }
        }
        "open" | "launch" => {
            let action = string_value("action").unwrap_or("open");
            if let Some(target) = string_value("target") {
                lines.push(format!("{action} {}", compact_tool_text(target, 96)));
            } else {
                lines.push(action.to_string());
            }
        }
        "todo" => {
            if let Some(count) = map
                .get("todos")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
            {
                lines.push(format!("{count} items"));
            }
        }
        "memory" | "goal" | "side_panel" | "bg" | "mcp" | "selfdev" | "swarm" => {
            if let Some(action) = string_value("action") {
                let target = string_value("title")
                    .or_else(|| string_value("id"))
                    .or_else(|| string_value("task_id"))
                    .or_else(|| string_value("server"))
                    .or_else(|| string_value("server_name"));
                lines.push(match target {
                    Some(target) => format!("{action} {}", compact_tool_text(target, 96)),
                    None => action.to_string(),
                });
            }
        }
        "batch" => {
            if let Some(count) = map
                .get("tool_calls")
                .and_then(serde_json::Value::as_array)
                .map(Vec::len)
            {
                lines.push(format!("{count} calls"));
            }
        }
        "subagent" | "task" => {
            let desc = string_value("description").unwrap_or("task");
            let agent_type = string_value("subagent_type").unwrap_or("agent");
            lines.push(format!(
                "{} ({})",
                compact_tool_text(desc, 84),
                compact_tool_text(agent_type, 28)
            ));
        }
        _ => {}
    }

    if bool_value("run_in_background") == Some(true) {
        lines.push("background: yes".to_string());
    }

    (!lines.is_empty()).then_some(lines)
}

pub(crate) fn tool_input_key_priority(key: &str) -> usize {
    match key {
        "command" => 0,
        "file_path" | "path" => 1,
        "query" => 2,
        "pattern" | "glob" => 3,
        "url" => 4,
        "action" => 5,
        "task" | "prompt" | "description" => 6,
        // Rendered as the card headline instead of a trailing key-value pair.
        "intent" => 90,
        _ => 100,
    }
}

pub(crate) fn compact_tool_json_value(key: &str, value: &serde_json::Value) -> String {
    if is_sensitive_tool_input_key(key) {
        return "••••".to_string();
    }
    match value {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => {
            if key.to_ascii_lowercase().contains("base64") {
                format!("<base64, {} chars>", value.chars().count())
            } else {
                compact_tool_text(value, 108)
            }
        }
        serde_json::Value::Array(values) => {
            if values.is_empty() {
                "[]".to_string()
            } else if values.len() <= 3 && values.iter().all(is_compact_tool_scalar) {
                let joined = values
                    .iter()
                    .map(|value| compact_tool_json_value(key, value))
                    .collect::<Vec<_>>()
                    .join(", ");
                compact_tool_text(&format!("[{joined}]"), 108)
            } else {
                format!("[{} items]", values.len())
            }
        }
        serde_json::Value::Object(map) => format!("{{{} fields}}", map.len()),
    }
}

pub(crate) fn is_compact_tool_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Null
            | serde_json::Value::Bool(_)
            | serde_json::Value::Number(_)
            | serde_json::Value::String(_)
    )
}

pub(crate) fn is_sensitive_tool_input_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("password") || key.contains("token") || key.contains("secret")
}

pub(crate) fn compact_tool_text(text: &str, max_chars: usize) -> String {
    let mut compacted = String::new();
    let mut chars = 0usize;
    let mut first_word = true;

    for word in text.split_whitespace() {
        if first_word {
            first_word = false;
        } else if chars == max_chars {
            compacted.push('…');
            return compacted;
        } else {
            compacted.push(' ');
            chars += 1;
        }

        for ch in word.chars() {
            if chars == max_chars {
                compacted.push('…');
                return compacted;
            }
            compacted.push(ch);
            chars += 1;
        }
    }

    compacted
}

pub(crate) fn normalized_tool_call_id(id: Option<String>) -> Option<String> {
    id.map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

pub(crate) fn merge_tool_finish_with_existing_context(existing: &str, finish_line: &str) -> String {
    let context = existing
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if context.is_empty() {
        finish_line.to_string()
    } else {
        format!("{}\n{}", finish_line, context.join("\n"))
    }
}

pub(crate) fn append_meta_lines(lines: &mut Vec<SingleSessionStyledLine>, content: &str) {
    if content.is_empty() {
        return;
    }
    lines.push(styled_line(
        format!("  {content}"),
        SingleSessionLineStyle::Meta,
    ));
}
