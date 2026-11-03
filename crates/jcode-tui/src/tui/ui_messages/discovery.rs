//! Compact discovery result cards.
use super::*;

pub(crate) fn split_discovery_blurb_url(value: &str) -> (String, Option<String>) {
    let value = value.trim();
    if let Some((blurb, url)) = value.rsplit_once(" (")
        && let Some(url) = url.strip_suffix(')')
        && url.starts_with("http")
    {
        return (blurb.trim().to_string(), Some(url.to_string()));
    }
    (value.to_string(), None)
}

pub(crate) fn parse_discovery_listing_names(output: &str) -> Vec<String> {
    output
        .lines()
        .filter_map(|line| {
            let (name, _) = line.trim().strip_prefix("- ")?.split_once(": ")?;
            Some(name.trim().to_string())
        })
        .collect()
}

pub(crate) fn discovery_selected_details(
    output: &str,
    name: &str,
) -> (Option<String>, Option<String>) {
    let prefix = format!("{name}: ");
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .map(split_discovery_blurb_url)
        .map(|(blurb, url)| (Some(blurb), url))
        .unwrap_or((None, None))
}

pub(crate) fn discovery_setup(output: &str) -> Option<String> {
    let (_, rest) = output.split_once("\n\nSetup: ")?;
    Some(
        rest.split("\n\nConsequential actions")
            .next()
            .unwrap_or(rest)
            .trim()
            .to_string(),
    )
    .filter(|value| !value.is_empty())
}

pub(crate) fn push_compact_discovery_kv(
    content: &mut Vec<Line<'static>>,
    label: &str,
    value: &str,
    available_width: usize,
    label_style: Style,
    value_style: Style,
    max_lines: usize,
) {
    let inner_width = available_width.saturating_sub(2).max(1);
    let mut wrapped = Vec::new();
    push_wrapped_kv_line(
        &mut wrapped,
        label,
        value,
        inner_width,
        label_style,
        value_style,
    );
    if wrapped.is_empty() {
        return;
    }

    let hidden = wrapped.len().saturating_sub(max_lines);
    let mut visible = wrapped.into_iter().take(max_lines).collect::<Vec<_>>();
    if hidden > 0
        && let Some(last) = visible.pop()
    {
        visible.push(super::truncate_line_preserving_suffix_to_width(
            &last,
            &Line::from(Span::styled(" …", value_style)),
            inner_width,
        ));
    }
    for mut line in visible {
        line.spans.insert(0, Span::raw("  "));
        content.push(line);
    }
}

pub(crate) fn push_compact_discovery_header(
    content: &mut Vec<Line<'static>>,
    spans: Vec<Span<'static>>,
    available_width: usize,
) {
    let mut line_spans = vec![Span::raw("  ")];
    line_spans.extend(spans);
    content.push(super::truncate_line_with_ellipsis_to_width(
        &Line::from(line_spans),
        available_width,
    ));
}

pub(crate) fn render_discovery_card(
    tool: &ToolCall,
    tool_output: &str,
    is_error: bool,
    available_width: usize,
) -> Option<Vec<Line<'static>>> {
    if tools_ui::canonical_tool_name(&tool.name) != "integration_tools" {
        return None;
    }
    let block_width = available_width.min(96);
    if block_width < 12 {
        return None;
    }
    if tool_output.trim().is_empty() || tool_output.trim() == tool.name {
        return None;
    }

    let muted_style = Style::default().fg(dim_color());
    let label_style = muted_style;
    let name_style = muted_style;
    let mut content = Vec::new();

    if is_error {
        return None;
    }

    let explicit_action = tool
        .input
        .get("action")
        .and_then(|value| value.as_str())
        .map(str::trim);
    let action = explicit_action.unwrap_or_else(|| {
        if tool
            .input
            .get("tool")
            .and_then(|value| value.as_str())
            .is_some()
        {
            "select"
        } else {
            "browse"
        }
    });

    match action {
        "suggest" => {
            let kind = tool
                .input
                .get("suggestion_kind")
                .and_then(|value| value.as_str())
                .unwrap_or("capability_gap");
            let kind_label = match kind {
                "known_product" => tool
                    .input
                    .get("product_name")
                    .and_then(|value| value.as_str())
                    .map(|name| format!("Known product · {name}"))
                    .unwrap_or_else(|| "Known product".to_string()),
                _ => "Capability gap".to_string(),
            };
            let receipt = if tool_output.starts_with("Catalog suggestion already recorded.") {
                "suggestion already recorded"
            } else {
                "suggestion sent"
            };
            push_compact_discovery_header(
                &mut content,
                vec![
                    Span::styled(receipt.to_string(), muted_style),
                    Span::styled(" · ", muted_style),
                    Span::styled(kind_label, name_style),
                ],
                block_width,
            );

            if let Some(reason) = tool.input.get("reason").and_then(|value| value.as_str()) {
                push_compact_discovery_kv(
                    &mut content,
                    "gap",
                    reason,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(url) = tool
                .input
                .get("product_url")
                .and_then(|value| value.as_str())
            {
                push_compact_discovery_kv(
                    &mut content,
                    "url",
                    url,
                    block_width,
                    label_style,
                    muted_style,
                    1,
                );
            }
            if let Some(requirements) = tool
                .input
                .get("requirements")
                .and_then(|value| value.as_array())
            {
                let requirements = requirements
                    .iter()
                    .filter_map(|value| value.as_str())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>();
                if !requirements.is_empty() {
                    push_compact_discovery_kv(
                        &mut content,
                        "needs",
                        &requirements.join(" · "),
                        block_width,
                        label_style,
                        muted_style,
                        MAX_DISCOVERY_DETAIL_LINES,
                    );
                }
            }
            push_compact_discovery_kv(
                &mut content,
                "review",
                "Jcode maintainers only; not approval or availability",
                block_width,
                label_style,
                muted_style,
                MAX_DISCOVERY_DETAIL_LINES,
            );
        }
        "select" | "setup" => {
            let name = tool
                .input
                .get("tool")
                .and_then(|value| value.as_str())
                .unwrap_or("tool");
            push_compact_discovery_header(
                &mut content,
                vec![
                    Span::styled(
                        if tool_output.starts_with("Selected off-catalog product") {
                            "selected off-catalog "
                        } else {
                            "selected "
                        },
                        muted_style,
                    ),
                    Span::styled(name.to_string(), name_style),
                ],
                block_width,
            );
            let (blurb, url) = discovery_selected_details(tool_output, name);
            let description = match (blurb.as_deref(), url.as_deref()) {
                (Some(blurb), Some(url)) => Some(format!("{blurb} · {url}")),
                (Some(blurb), None) => Some(blurb.to_string()),
                (None, Some(url)) => Some(url.to_string()),
                (None, None) => None,
            };
            if let Some(description) = description {
                push_compact_discovery_kv(
                    &mut content,
                    "details",
                    &description,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(reason) = tool.input.get("reason").and_then(|value| value.as_str()) {
                push_compact_discovery_kv(
                    &mut content,
                    "why",
                    reason,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_DETAIL_LINES,
                );
            }
            if let Some(setup) = discovery_setup(tool_output) {
                push_compact_discovery_kv(
                    &mut content,
                    "setup",
                    &setup,
                    block_width,
                    label_style,
                    muted_style,
                    MAX_DISCOVERY_SETUP_LINES,
                );
            }
        }
        _ => {
            // Browse results render as a single compact line: the entry name
            // when exactly one matched, a count otherwise, and nothing at all
            // when the catalog had no matches.
            let entries = parse_discovery_listing_names(tool_output);
            match entries.len() {
                0 => return None,
                1 => {
                    push_compact_discovery_header(
                        &mut content,
                        vec![Span::styled(entries[0].clone(), name_style)],
                        block_width,
                    );
                }
                count => {
                    push_compact_discovery_header(
                        &mut content,
                        vec![Span::styled(format!("{count} integrations"), muted_style)],
                        block_width,
                    );
                }
            }
        }
    }

    Some(content)
}
