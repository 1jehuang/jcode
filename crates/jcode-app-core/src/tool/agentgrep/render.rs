use super::*;

const MAX_RENDERED_MATCH_LINE_CHARS: usize = 240;
const RENDERED_MATCH_PREFIX_CONTEXT_CHARS: usize = 80;
const MAX_NON_CODE_MATCH_LINES_PER_FILE: usize = 3;

pub(super) fn render_grep_output(
    result: &GrepResult,
    args: &GrepArgs,
    max_matches: Option<usize>,
) -> String {
    if args.paths_only {
        return result
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut lines = vec![
        format!("query: {}", result.query),
        format!(
            "matches: {} in {} files",
            result.total_matches, result.total_files
        ),
    ];
    let mut state = GrepRenderState::new(max_matches);

    for file in &result.files {
        if state.limit_reached() {
            break;
        }
        render_grep_file(file, args, &mut lines, &mut state);
    }

    if let Some(max) = max_matches
        && result.total_matches > state.displayed_matches
    {
        lines.push(String::new());
        lines.push(format!(
            "... {} more matches omitted (max_regions={})",
            result.total_matches.saturating_sub(state.displayed_matches),
            max
        ));
    }

    lines.join("\n")
}

struct GrepRenderState {
    displayed_matches: usize,
    max_matches: Option<usize>,
}

impl GrepRenderState {
    fn new(max_matches: Option<usize>) -> Self {
        Self {
            displayed_matches: 0,
            max_matches,
        }
    }

    fn limit_reached(&self) -> bool {
        self.max_matches
            .is_some_and(|max| self.displayed_matches >= max)
    }

    fn remaining_matches(&self) -> usize {
        self.max_matches
            .map(|max| max.saturating_sub(self.displayed_matches))
            .unwrap_or(usize::MAX)
    }

    fn record_match(&mut self) {
        self.displayed_matches += 1;
    }
}

fn render_grep_file(
    file: &FileMatches,
    args: &GrepArgs,
    lines: &mut Vec<String>,
    state: &mut GrepRenderState,
) {
    lines.push(String::new());
    lines.push(file.path.clone());
    if file.total_symbols > 0 {
        lines.push(format!(
            "  symbols: {} total, {} matched, {} other",
            file.total_symbols,
            file.matched_symbol_count,
            file.total_symbols.saturating_sub(file.matched_symbol_count)
        ));
    } else {
        lines.push("  symbols: no structural items detected".to_string());
    }
    let non_code_cap = non_code_match_cap(file);
    let mut file_displayed_matches = 0usize;

    for group in &file.groups {
        if state.limit_reached() {
            break;
        }
        let remaining_file_matches = non_code_cap
            .map(|cap| cap.saturating_sub(file_displayed_matches))
            .unwrap_or(usize::MAX);
        let remaining_matches = state.remaining_matches().min(remaining_file_matches);
        if remaining_matches == 0 {
            break;
        }
        let visible_matches = group
            .resolved_matches(&file.matches)
            .take(remaining_matches)
            .collect::<Vec<_>>();
        if visible_matches.is_empty() {
            continue;
        }

        match (group.start_line, group.end_line) {
            (Some(start_line), Some(end_line)) => lines.push(format!(
                "    - {} {} @ {}-{}",
                group.kind, group.label, start_line, end_line
            )),
            _ => lines.push(format!("    - {}", group.label)),
        }
        for line_match in visible_matches {
            let line_text = compact_rendered_match_line(&line_match.line_text, args);
            lines.push(format!(
                "      - @ {} {}",
                line_match.line_number, line_text
            ));
            file_displayed_matches += 1;
            state.record_match();
        }
    }
    if non_code_cap.is_some()
        && !state.limit_reached()
        && file.matches.len() > file_displayed_matches
    {
        lines.push(format!(
            "    - ... {} more non-code matches omitted; narrow path/glob/type or use paths_only for full file list",
            file.matches.len().saturating_sub(file_displayed_matches)
        ));
    }
    if !file.other_symbols.is_empty() {
        let mut summary = file
            .other_symbols
            .iter()
            .map(|item| {
                format!(
                    "{} {} @ {}-{}",
                    item.kind, item.label, item.start_line, item.end_line
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        if file.other_symbols_omitted_count > 0 {
            if !summary.is_empty() {
                summary.push_str("; ");
            }
            summary.push_str(&format!("... {} more", file.other_symbols_omitted_count));
        }
        lines.push(format!("    - other: {summary}"));
    }
}

fn non_code_match_cap(file: &FileMatches) -> Option<usize> {
    match file.language.as_str() {
        "json" | "yaml" | "markdown" | "text" | "" => Some(MAX_NON_CODE_MATCH_LINES_PER_FILE),
        _ => None,
    }
}

pub(super) fn compact_rendered_match_line(line: &str, args: &GrepArgs) -> String {
    let char_count = line.chars().count();
    if char_count <= MAX_RENDERED_MATCH_LINE_CHARS {
        return line.to_string();
    }

    let match_start_char = if args.regex {
        0
    } else {
        args.query
            .is_empty()
            .then_some(0)
            .or_else(|| {
                line.find(&args.query)
                    .map(|byte| line[..byte].chars().count())
            })
            .unwrap_or(0)
    };
    let start_char = match_start_char.saturating_sub(RENDERED_MATCH_PREFIX_CONTEXT_CHARS);
    let end_char = start_char
        .saturating_add(MAX_RENDERED_MATCH_LINE_CHARS)
        .min(char_count);
    let start_char = end_char
        .saturating_sub(MAX_RENDERED_MATCH_LINE_CHARS)
        .min(start_char);

    let omitted_prefix = start_char;
    let omitted_suffix = char_count.saturating_sub(end_char);
    let snippet: String = line
        .chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect();

    match (omitted_prefix > 0, omitted_suffix > 0) {
        (true, true) => format!(
            "…{} … [truncated: {} chars before, {} chars after]",
            snippet, omitted_prefix, omitted_suffix
        ),
        (true, false) => format!("…{} [truncated: {} chars before]", snippet, omitted_prefix),
        (false, true) => format!("{} … [truncated: {} chars after]", snippet, omitted_suffix),
        (false, false) => snippet,
    }
}

pub(super) fn render_find_output(result: &FindResult, args: &FindArgs) -> String {
    if args.paths_only {
        return result
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut lines = vec![
        format!("query: {}", result.query),
        format!("top files: {}", result.files.len()),
    ];

    for (idx, file) in result.files.iter().enumerate() {
        render_find_file(idx, file, args, &mut lines);
    }

    lines.join("\n")
}

fn render_find_file(idx: usize, file: &FindFile, args: &FindArgs, lines: &mut Vec<String>) {
    lines.push(String::new());
    lines.push(format!("{}. {}", idx + 1, file.path));
    lines.push(format!("   role: {}", file.role));
    lines.push("   why:".to_string());
    for reason in &file.why {
        lines.push(format!("     - {reason}"));
    }
    if args.debug_score {
        lines.push(format!("   score: {}", file.score));
    }
    lines.push("   structure:".to_string());
    for item in &file.structure.items {
        lines.push(format!(
            "     - {} {} @ {}-{} ({} lines)",
            item.kind, item.label, item.start_line, item.end_line, item.line_count
        ));
    }
    if file.structure.omitted_count > 0 {
        lines.push(format!(
            "     ... {} more symbols",
            file.structure.omitted_count
        ));
    }
}

pub(super) fn render_outline_output(result: &OutlineResult) -> String {
    let mut lines = vec![
        format!("file: {}", result.path),
        format!("language: {}", result.language),
        format!("role: {}", result.role),
        format!("lines: {}", result.total_lines),
        format!(
            "symbols: {}",
            result.structure.items.len() + result.structure.omitted_count
        ),
        String::new(),
        "structure:".to_string(),
    ];

    if result.structure.items.is_empty() {
        lines.push("  (no structural items detected)".to_string());
    } else {
        for item in &result.structure.items {
            lines.push(format!(
                "  - {} {} @ {}-{} ({} lines)",
                item.kind, item.label, item.start_line, item.end_line, item.line_count
            ));
        }
        if result.structure.omitted_count > 0 {
            lines.push(format!(
                "  ... {} more symbols",
                result.structure.omitted_count
            ));
        }
    }
    if let Some(note) = &result.context_applied {
        lines.push(String::new());
        lines.push(format!("context: {note}"));
    }

    lines.join("\n")
}

pub(super) fn render_smart_output(result: &SmartResult, args: &SmartArgs) -> String {
    if args.paths_only {
        return result
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut lines = Vec::new();
    if args.debug_plan {
        lines.extend(render_debug_plan(result));
        lines.push(String::new());
    }
    lines.push("query parameters:".to_string());
    lines.push(format!("  subject: {}", result.query.subject));
    lines.push(format!("  relation: {}", result.query.relation.as_str()));
    if !result.query.support.is_empty() {
        lines.push(format!("  support: {}", result.query.support.join(", ")));
    }
    if let Some(kind) = &result.query.kind {
        lines.push(format!("  kind: {kind}"));
    }
    if let Some(path_hint) = &result.query.path_hint {
        lines.push(format!("  path_hint: {path_hint}"));
    }
    lines.push(String::new());
    lines.push(format!(
        "top results: {} files, {} regions",
        result.summary.total_files, result.summary.total_regions
    ));
    if result.files.is_empty() {
        lines.push("no results found for the current trace query and scope".to_string());
    }
    if let Some(best_file) = &result.summary.best_file {
        lines.push(format!("best answer likely in {best_file}"));
    }
    for (idx, file) in result.files.iter().enumerate() {
        render_smart_file(idx, file, args, &mut lines);
    }

    lines.join("\n")
}

fn render_debug_plan(result: &SmartResult) -> Vec<String> {
    let relation_terms = match result.query.relation {
        Relation::Rendered => "render, draw, ui, widget, view",
        Relation::CalledFrom => "call, invoke, dispatch",
        Relation::TriggeredFrom => "trigger, dispatch, schedule",
        Relation::Populated => "set, assign, insert, push, build",
        Relation::ComesFrom => "source, load, parse, read, fetch",
        Relation::Handled => "handle, handler, event, dispatch",
        Relation::Defined => "fn, struct, enum, class, def",
        Relation::Implementation => "impl, register, wire, tool",
        _ => result.query.relation.as_str(),
    };
    let mut lines = vec![
        "debug plan:".to_string(),
        "  mode: trace".to_string(),
        format!("  subject: {}", result.query.subject),
        format!("  relation: {}", result.query.relation.as_str()),
        format!("  relation_terms: {relation_terms}"),
    ];
    if let Some(kind) = &result.query.kind {
        lines.push(format!("  kind filter: {kind}"));
    }
    if let Some(path_hint) = &result.query.path_hint {
        lines.push(format!("  path hint: {path_hint}"));
    }
    if !result.query.support.is_empty() {
        lines.push(format!(
            "  support terms: {}",
            result.query.support.join(", ")
        ));
    }
    lines
}

fn render_smart_file(idx: usize, file: &SmartFile, args: &SmartArgs, lines: &mut Vec<String>) {
    lines.push(String::new());
    lines.push(format!("{}. {}", idx + 1, file.path));
    lines.push(format!("   role: {}", file.role));
    lines.push("   why:".to_string());
    for reason in &file.why {
        lines.push(format!("     - {reason}"));
    }
    if args.debug_score {
        lines.push(format!("   score: {}", file.score));
    }
    lines.push("   structure:".to_string());
    for item in &file.structure.items {
        lines.push(format!(
            "     - {} {} @ {}-{} ({} lines)",
            item.kind, item.label, item.start_line, item.end_line, item.line_count
        ));
    }
    if file.structure.omitted_count > 0 {
        lines.push(format!(
            "     ... {} more symbols",
            file.structure.omitted_count
        ));
    }
    if let Some(note) = &file.context_applied {
        lines.push(format!("   context: {note}"));
    }
    lines.push("   regions:".to_string());
    for region in &file.regions {
        render_smart_region(region, args.debug_score, lines);
    }
}

fn render_smart_region(region: &SmartRegion, debug_score: bool, lines: &mut Vec<String>) {
    lines.push(format!(
        "     - {} @ {}-{} ({} lines)",
        region.label, region.start_line, region.end_line, region.line_count
    ));
    lines.push(format!("       kind: {}", region.kind));
    if debug_score {
        lines.push(format!("       score: {}", region.score));
    }
    if region.full_region {
        lines.push("       full region:".to_string());
    } else {
        lines.push("       snippet:".to_string());
    }
    for line in region.body.lines() {
        lines.push(format!("         {line}"));
    }
    lines.push("       why:".to_string());
    for reason in &region.why {
        lines.push(format!("         - {reason}"));
    }
    if let Some(note) = &region.context_applied {
        lines.push(format!("       context: {note}"));
    }
}
