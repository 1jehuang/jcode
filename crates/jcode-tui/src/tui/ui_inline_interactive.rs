use super::*;
use jcode_tui_style::theme::warning_color;
use ratatui::widgets::{Block, BorderType, Borders};
use unicode_width::UnicodeWidthStr;

fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

fn truncate_display(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }
    out.push('…');
    out
}

fn pad_left_display(text: &str, width: usize) -> String {
    let truncated = truncate_display(text, width);
    let padding = width.saturating_sub(display_width(truncated.as_str()));
    format!("{}{}", truncated, " ".repeat(padding))
}

fn api_method_display(raw: &str) -> String {
    let method = crate::provider::ModelRouteApiMethod::parse(raw).display_label();
    match jcode_provider_core::oauth_account_label(raw) {
        Some(label) => format!("{method} · {label}"),
        None => method,
    }
}

fn route_method_display(route: &crate::tui::PickerOption) -> String {
    let Some(label) = jcode_provider_core::oauth_account_label(&route.api_method) else {
        return api_method_display(&route.api_method);
    };
    // Only the catalog identity prefix is authoritative, not later diagnostics.
    let identity = format!("account {label} · ");
    let detail = route
        .detail
        .trim_start_matches("[global] ")
        .trim_start_matches("[session] ");
    let email = detail
        .strip_prefix(&identity)
        .and_then(|rest| rest.split(" · ").next())
        .filter(|email| email.contains('@') && !email.chars().any(char::is_whitespace));
    match email {
        Some(email) => format!("oauth · {email}"),
        None => api_method_display(&route.api_method),
    }
}

fn route_provider_display(provider: &str, api_method: &str) -> String {
    if crate::provider::ModelRouteApiMethod::parse(api_method).is_openrouter()
        && provider != "auto"
        && !provider.contains("OpenRouter")
    {
        format!("OpenRouter/{}", provider)
    } else {
        provider.to_string()
    }
}

fn picker_entry_display_name(entry: &crate::tui::PickerEntry) -> String {
    let base = picker_entry_pretty_name(entry);
    let default_marker = if entry.is_default { " default" } else { "" };
    let is_new = entry
        .options
        .iter()
        .any(|option| option.detail.contains("recently added"));
    let suffix = if is_new && !entry.is_current {
        format!(" new{}", default_marker)
    } else if entry.is_favorite {
        format!(" ♥{}", default_marker)
    } else if entry.recommended {
        format!(" ★{}", default_marker)
    } else if entry.old && !entry.is_current {
        if let Some(ref date) = entry.created_date {
            format!(" {}{}", date, default_marker)
        } else {
            format!(" old{}", default_marker)
        }
    } else if let Some(ref date) = entry.created_date {
        if !entry.is_current {
            format!(" {}{}", date, default_marker)
        } else {
            default_marker.to_string()
        }
    } else {
        default_marker.to_string()
    };

    format!("{}{}", base, suffix)
}

/// Human-friendly rendering of a model picker row's model name.
///
/// `/model` rows historically showed the raw provider model id
/// (`claude-opus-4-8`), which reads worse than the pretty names every other
/// surface uses (header, status line, info widgets). We prettify only the
/// well-known families so unfamiliar or namespaced ids (OpenRouter
/// `vendor/model`, local profiles) keep their exact spelling and stay
/// copy-pasteable.
fn picker_entry_pretty_name(entry: &crate::tui::PickerEntry) -> String {
    if !matches!(
        entry.action,
        crate::tui::PickerAction::Model | crate::tui::PickerAction::AgentModelChoice { .. }
    ) {
        return entry.name.clone();
    }
    crate::tui::app::helpers::model_names::pretty_known_model_family(&entry.name)
        .unwrap_or_else(|| entry.name.clone())
}

fn picker_row_marker(is_row_selected: bool, unavailable: bool, limited: bool) -> &'static str {
    if unavailable {
        "×"
    } else if limited {
        "⚠"
    } else if is_row_selected {
        "▸"
    } else {
        " "
    }
}

fn route_detail_display_text(detail: &str, unavailable: bool) -> Option<String> {
    let trimmed = detail.trim();
    if unavailable {
        if trimmed.is_empty() {
            Some("unavailable".to_string())
        } else {
            Some(format!("unavailable · {}", trimmed))
        }
    } else if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn route_detail_is_limited(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("fallback:")
        || lower.contains("fallback model")
        || lower.contains("no tools")
        || lower.contains("requires an inference profile")
        || lower.contains("catalog still loading")
        || lower.contains("provider will initialize")
}

fn selected_route_notice_text(
    picker: &crate::tui::InlineInteractiveState,
    route: Option<&crate::tui::PickerOption>,
) -> Option<(String, bool)> {
    if picker.kind != crate::tui::PickerKind::Model {
        return None;
    }
    let route = route?;
    let unavailable = !route.available;
    let detail = route_detail_display_text(&route.detail, unavailable)?;
    if unavailable {
        return Some((format!("× {}", detail), true));
    }
    if route_detail_is_limited(&route.detail) {
        return Some((format!("⚠ {}", detail), true));
    }
    if route
        .detail
        .to_ascii_lowercase()
        .contains("inference profile")
    {
        return Some((format!("ⓘ {}", detail), false));
    }
    None
}

fn model_picker_top_hint(picker: &crate::tui::InlineInteractiveState) -> Option<&'static str> {
    if picker.selected_entry().is_some_and(|entry| {
        matches!(
            entry.action,
            crate::tui::PickerAction::AgentProfiles
                | crate::tui::PickerAction::AgentProfileChoice(_)
        )
    }) {
        return Some(
            " INSTRUCTION PROFILE · THIS SESSION ONLY · Enter choose sources · Esc cancel · no global default",
        );
    }
    if picker.kind == crate::tui::PickerKind::Account {
        use crate::tui::{AccountPickerAction, PickerAction};
        return Some(match picker.selected_entry().map(|entry| &entry.action) {
            Some(PickerAction::Account(AccountPickerAction::Switch { provider_id, .. }))
                if provider_id == "claude-code" =>
            {
                " Enter use in this window · default: config.toml · type to search"
            }
            Some(PickerAction::Account(AccountPickerAction::Switch { .. })) => {
                " Enter use in this window · Ctrl+D default for new windows · type to search"
            }
            Some(PickerAction::Account(AccountPickerAction::Add { .. })) => {
                " Enter sign in to another account · type to search"
            }
            Some(PickerAction::Account(AccountPickerAction::Replace { .. })) => {
                " Enter refresh this saved login · type to search"
            }
            Some(PickerAction::Account(AccountPickerAction::OpenCenter { .. })) => {
                " Enter open account center · type to search"
            }
            Some(PickerAction::Account(AccountPickerAction::SetDefault { .. })) => {
                " Enter default for new/unpinned windows · type to search"
            }
            None => " Type to search · Esc clear filter",
            _ => " Enter open selected action · type to search",
        });
    }
    let is_swarm_agent_model_picker = picker.kind == crate::tui::PickerKind::Model
        && picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::AgentModelChoice {
                    target: crate::tui::AgentModelTarget::Swarm,
                    ..
                }
            )
        });
    if let Some(global) = agent_models_scope(picker) {
        return Some(match (global, is_swarm_agent_model_picker) {
            (true, false) => {
                " GLOBAL DEFAULT: changes every session without its own choice · Ctrl+G this session instead"
            }
            (false, false) => {
                " THIS SESSION ONLY: other sessions keep the global default · Ctrl+G global default"
            }
            (true, true) => {
                " GLOBAL DEFAULT (all sessions) · Ctrl+G this session · swarm routing: /swarm-prompt"
            }
            (false, true) => {
                " THIS SESSION ONLY · Ctrl+G global default · swarm routing: /swarm-prompt"
            }
        });
    }

    // The favorite/default hotkeys now work in both the focused picker and the
    // as-you-type preview, so the hint is shown whenever this is a runtime model
    // picker (i.e. it has selectable model rows).
    let is_runtime_model_picker = picker.kind == crate::tui::PickerKind::Model
        && picker
            .entries
            .iter()
            .any(|entry| matches!(entry.action, crate::tui::PickerAction::Model));
    if is_runtime_model_picker {
        Some(
            " keys: Ctrl+O set default · Ctrl+N favorite · Shift+Tab switch active model to next favorite",
        )
    } else {
        None
    }
}

/// Scope of an open `/agents` view: `Some(true)` edits the global default,
/// `Some(false)` this session, `None` when it is not an `/agents` view. Both
/// views tag every row with its scope (`Swarm [session]` role rows,
/// `[global] ...` model route details), so the scope is read from there.
fn agent_models_scope(picker: &crate::tui::InlineInteractiveState) -> Option<bool> {
    if picker.kind != crate::tui::PickerKind::Model || picker.effort_step.is_some() {
        return None;
    }
    let is_agents_view = picker.is_agent_target_picker()
        || picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::AgentModelChoice { .. }
            )
        });
    if !is_agents_view {
        return None;
    }
    Some(picker.entries.iter().any(|entry| {
        entry.name.ends_with("[global]")
            || entry
                .options
                .iter()
                .any(|option| option.detail.starts_with("[global]"))
    }))
}

fn account_picker_shows_provider_badge(picker: &crate::tui::InlineInteractiveState) -> bool {
    let mut providers: Vec<&str> = Vec::new();
    for &fi in &picker.filtered {
        let entry = &picker.entries[fi];
        if let Some(route) = entry.options.get(entry.selected_option) {
            let provider = route.provider.trim();
            if !provider.is_empty()
                && !providers
                    .iter()
                    .any(|existing| existing.eq_ignore_ascii_case(provider))
            {
                providers.push(provider);
                if providers.len() > 1 {
                    return true;
                }
            }
        }
    }
    false
}

fn account_picker_entry_title(
    entry: &crate::tui::PickerEntry,
    show_provider_badge: bool,
) -> (String, usize) {
    let display_name = picker_entry_display_name(entry);
    let provider_prefix = if show_provider_badge {
        entry
            .options
            .get(entry.selected_option)
            .map(|route| format!("{} · ", route.provider))
            .unwrap_or_default()
    } else {
        String::new()
    };
    let prefix_chars = provider_prefix.chars().count();
    (format!("{}{}", provider_prefix, display_name), prefix_chars)
}

fn account_inline_interactive_state_label(entry: &crate::tui::PickerEntry) -> &'static str {
    entry.account_state_label().unwrap_or("-")
}

fn picker_render_width(picker: &crate::tui::InlineInteractiveState, max_width: usize) -> usize {
    let marker_width = 3usize;
    let is_preview = picker.preview;
    const WIDTH_SCAN_LIMIT: usize = 200;

    if picker.uses_compact_navigation() {
        let show_provider_badge = account_picker_shows_provider_badge(picker);
        let mut max_title_len = display_width("ACCOUNT");
        let mut max_state_len = display_width("STATE");

        for &fi in &picker.filtered {
            let entry = &picker.entries[fi];
            let (title, _) = account_picker_entry_title(entry, show_provider_badge);
            max_title_len = max_title_len.max(display_width(title.as_str()));
            max_state_len =
                max_state_len.max(display_width(account_inline_interactive_state_label(entry)));
        }

        let state_width = (max_state_len + 1).clamp(7, 10);
        let min_title_width = max_title_len.clamp(8, 10);
        let title_cap = if picker.kind == crate::tui::PickerKind::Account {
            max_title_len
        } else if show_provider_badge {
            42
        } else {
            34
        };
        let budget = max_width.saturating_sub(marker_width + state_width);
        let title_width = max_title_len
            .min(title_cap)
            .min(budget.max(min_title_width.min(budget)));

        return marker_width + title_width + state_width;
    }

    let mut max_model_len = display_width(picker.primary_label());
    let mut max_provider_len = display_width(picker.secondary_label(is_preview));
    let mut max_via_len = display_width(picker.tertiary_label());

    for &fi in picker.filtered.iter().take(WIDTH_SCAN_LIMIT) {
        let entry = &picker.entries[fi];
        max_model_len = max_model_len.max(display_width(picker_entry_display_name(entry).as_str()));
        if let Some(route) = entry.active_option() {
            let provider_label = route_provider_display(&route.provider, &route.api_method);
            let provider_label = if entry.option_count() > 1 {
                format!("{} ({})", provider_label, entry.option_count())
            } else {
                provider_label
            };
            max_provider_len = max_provider_len.max(display_width(provider_label.as_str()));
            max_via_len = max_via_len.max(display_width(&route_method_display(route)));
        }
    }

    let mut provider_width = max_provider_len + 1;
    let mut via_width = max_via_len + 1;
    let model_cap = if picker.kind == crate::tui::PickerKind::Model {
        max_width
    } else if is_preview {
        42
    } else {
        56
    };
    let min_model_width = max_model_len.clamp(6, 16);

    let budget = max_width.saturating_sub(marker_width);
    if provider_width + via_width + min_model_width > budget {
        let provider_floor = 8usize.min(provider_width);
        let via_floor = 4usize.min(via_width);

        let provider_reduction = (provider_width + via_width + min_model_width)
            .saturating_sub(budget)
            .min(provider_width.saturating_sub(provider_floor));
        provider_width = provider_width.saturating_sub(provider_reduction);

        let via_reduction = (provider_width + via_width + min_model_width)
            .saturating_sub(budget)
            .min(via_width.saturating_sub(via_floor));
        via_width = via_width.saturating_sub(via_reduction);
    }

    let model_budget = budget.saturating_sub(provider_width + via_width);
    let model_width = max_model_len
        .min(model_cap)
        .min(model_budget.max(min_model_width.min(model_budget)));

    marker_width + provider_width + via_width + model_width
}

pub(super) fn format_elapsed(secs: f32) -> String {
    if secs >= 3600.0 {
        let hours = (secs / 3600.0) as u32;
        let mins = ((secs % 3600.0) / 60.0) as u32;
        format!("{}h {}m", hours, mins)
    } else if secs >= 60.0 {
        let mins = (secs / 60.0) as u32;
        let s = (secs % 60.0) as u32;
        format!("{}m {}s", mins, s)
    } else {
        format!("{}s", secs as u32)
    }
}

fn fuzzy_match_positions(pattern: &str, text: &str) -> Vec<usize> {
    jcode_fuzzy::fuzzy_match_token_positions(pattern, text)
}

/// Rows the focused `/model` browser may use (the as-you-type preview keeps
/// the command-suggestion limit).
const MODEL_BROWSER_VISIBLE_LIMIT: usize = 16;

/// Profile options are a draft step of the existing picker, never model routes.
pub(super) fn agent_profile_source_lines(
    picker: &crate::tui::InlineInteractiveState,
    available_rows: usize,
    width: usize,
) -> Vec<Line<'static>> {
    use crate::tui::PickerAction;
    let dim = Style::default().fg(dim_color());
    let accent = Style::default().fg(rgb(255, 213, 128));
    let Some(apply) = picker
        .entries
        .iter()
        .find(|entry| matches!(entry.action, PickerAction::AgentProfileApply(_)))
    else {
        return Vec::new();
    };
    let PickerAction::AgentProfileApply(name) = &apply.action else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    let mut append = |text: &str, style| {
        lines.extend(
            profile_source_wrap(text, width)
                .into_iter()
                .map(|line| Line::from(Span::styled(line, style))),
        );
    };
    append(
        &format!("Instruction sources: {name} · this session only"),
        accent,
    );
    append(
        "Profile alone by default. Optional Jcode-loaded Markdown stays OFF unless selected.",
        dim,
    );
    append(
        "Base instructions, tool protocol, skills and runtime context remain.",
        dim,
    );
    if let Some(option) = apply.active_option() {
        if !option.provider.is_empty() {
            append(&option.provider, dim);
        }
        if !option.detail.is_empty() {
            append(&option.detail, Style::default().fg(warning_color()));
        }
    }
    let footer = "↑↓ navigate · Space toggle · Enter select/apply · Esc cancel · no global default";
    // Preserve room for the focused row and controls even on short terminals.
    lines.truncate(available_rows.saturating_sub(4));
    let sources = picker.entries.len().saturating_sub(2);
    let visible = available_rows.saturating_sub(lines.len() + 3).min(sources);
    let selected = picker.selected.min(picker.entries.len().saturating_sub(1));
    if visible > 0 {
        let start = selected
            .min(sources.saturating_sub(1))
            .saturating_sub(visible.saturating_sub(1))
            .min(sources.saturating_sub(visible));
        for row in start..start + visible {
            let entry = &picker.entries[row];
            let focused = row == selected;
            let style = if focused { accent } else { dim };
            let detail = if matches!(entry.action, PickerAction::AgentProfileSource(_)) {
                entry
                    .active_option()
                    .map(|option| format!(" · {}", option.detail))
                    .unwrap_or_default()
            } else {
                String::new()
            };
            lines.push(Line::from(Span::styled(
                format!("{} {}{detail}", if focused { "▸" } else { " " }, entry.name),
                style,
            )));
        }
    }
    for row in sources..picker.entries.len() {
        if lines.len() >= available_rows {
            break;
        }
        lines.push(Line::from(Span::styled(
            format!(
                "{} {}",
                if row == selected { "▸" } else { " " },
                picker.entries[row].name
            ),
            if row == selected { accent } else { dim },
        )));
    }
    if lines.len() < available_rows {
        lines.push(Line::from(Span::styled(footer, dim.italic())));
    }
    lines
}

fn profile_source_wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && display_width(&line) + 1 + display_width(word) > width {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            for ch in word.chars() {
                if display_width(&line) + unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0)
                    > width
                    && !line.is_empty()
                {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

/// The reasoning-level step of `/model`: a header naming the model and route,
/// then one row per level.
fn model_effort_step_lines(
    picker: &crate::tui::InlineInteractiveState,
    step: &crate::tui::ModelEffortStep,
    available_rows: usize,
) -> Vec<Line<'static>> {
    let dim = Style::default().fg(dim_color());
    let accent = Style::default().fg(rgb(255, 213, 128));
    let mut lines = Vec::new();
    let mut header = vec![
        Span::styled(
            if step.save_default {
                "Default reasoning for "
            } else {
                "Reasoning for "
            },
            dim,
        ),
        Span::styled(picker_entry_pretty_name(&step.model), accent),
    ];
    if let Some(route) = step.model.active_option() {
        header.push(Span::styled(
            format!(
                "  via {} · {}",
                route_provider_display(&route.provider, &route.api_method),
                api_method_display(&route.api_method)
            ),
            dim,
        ));
    }
    lines.push(Line::from(header));
    let hint_rows = usize::from(available_rows > 2);
    let visible = available_rows
        .saturating_sub(1 + hint_rows)
        .min(picker.filtered.len());
    if visible == 0 {
        lines.truncate(available_rows);
        return lines;
    }
    let selected = picker.selected.min(picker.filtered.len() - 1);
    let start = selected.saturating_sub(visible - 1);
    for row in start..start + visible {
        let entry = &picker.entries[picker.filtered[row]];
        let style = if row == selected {
            accent
        } else {
            Style::default().fg(rgb(128, 203, 196))
        };
        let mut spans = vec![Span::styled(
            format!(
                "{} {}",
                picker_row_marker(row == selected, false, false),
                entry.name
            ),
            style,
        )];
        if entry.is_current {
            spans.push(Span::styled(" current", dim));
        }
        if entry.recommended {
            spans.push(Span::styled(" ★", dim));
        }
        lines.push(Line::from(spans));
    }
    if hint_rows > 0 {
        let confirm = if step.save_default {
            "Enter save default"
        } else {
            "Enter switch"
        };
        lines.push(Line::from(Span::styled(
            format!("↑↓ level · {confirm} · Esc back to models"),
            dim,
        )));
    }
    lines
}

/// Compact, borderless model choices for the command-suggestion surface.
/// Uses the existing picker state so filtering, routes and hotkeys are unchanged.
pub(super) fn model_suggestion_lines(
    picker: &crate::tui::InlineInteractiveState,
    available_rows: usize,
) -> Vec<Line<'static>> {
    if available_rows == 0 {
        return Vec::new();
    }
    if picker.is_agent_profile_sources() {
        return agent_profile_source_lines(picker, available_rows, 120);
    }
    if let Some(step) = picker.effort_step.as_deref() {
        return model_effort_step_lines(picker, step, available_rows);
    }
    let dim = Style::default().fg(dim_color());
    // The focused browser (bare `/model` + Enter) owns typing, so show its
    // search box and provider scope above the list.
    let browsing = !picker.preview
        && picker
            .entries
            .iter()
            .any(|entry| matches!(entry.action, crate::tui::PickerAction::Model));
    let mut search_line = None;
    if browsing {
        let (_, search) = crate::tui::app::split_model_picker_filter(&picker.filter);
        let provider = crate::tui::app::model_picker_active_provider(picker)
            .unwrap_or_else(|| "all providers".to_string());
        let accent = Style::default().fg(rgb(255, 213, 128));
        let mut spans = vec![
            Span::styled("Search: ", dim),
            Span::styled(format!("{search}▏"), accent),
            Span::styled("  Provider: ", dim),
            Span::styled(provider, accent),
            Span::styled(
                format!("  ({}/{})", picker.filtered.len(), picker.entries.len()),
                dim,
            ),
        ];
        if search.is_empty() {
            spans.insert(1, Span::styled("type to filter ", dim.italic()));
        }
        search_line = Some(Line::from(spans));
    }
    let search_rows = usize::from(search_line.is_some() && available_rows > 1);
    let available_rows = available_rows - search_rows;
    if picker.filtered.is_empty() {
        let message = if picker.filter.is_empty() {
            "No matching models".to_string()
        } else {
            format!("No matching models for {}", picker.filter)
        };
        let mut lines: Vec<Line<'static>> = search_line.into_iter().take(search_rows).collect();
        lines.push(Line::from(Span::styled(message, dim)));
        if browsing && available_rows > 1 {
            lines.push(Line::from(Span::styled(
                "Backspace edit search · Ctrl+P next provider · Esc clear",
                dim,
            )));
        }
        return lines;
    }
    let selected = picker.selected.min(picker.filtered.len() - 1);
    let selected_entry = &picker.entries[picker.filtered[selected]];
    let notice = selected_route_notice_text(picker, selected_entry.active_option());
    let hint = model_picker_top_hint(picker);
    let hint_rows = usize::from(available_rows > 1);
    let notice_rows = usize::from(notice.is_some() && available_rows > hint_rows + 1);
    let visible_limit = if browsing {
        MODEL_BROWSER_VISIBLE_LIMIT
    } else {
        crate::tui::app::COMMAND_SUGGESTION_VISIBLE_LIMIT
    };
    let visible = visible_limit
        .min(available_rows - hint_rows - notice_rows)
        .min(picker.filtered.len());
    let start = selected.saturating_sub(visible - 1);
    // Measure the visible rows, not the whole catalog: offscreen long names
    // should not push the provider and method out of the suggestion surface.
    let mut model_width = 0;
    let mut provider_width = 0;
    for &index in &picker.filtered[start..start + visible] {
        let entry = &picker.entries[index];
        model_width = model_width.max(
            display_width(&picker_entry_display_name(entry))
                + if entry.is_current {
                    " current".len()
                } else {
                    0
                },
        );
        if let Some(route) = entry.active_option() {
            provider_width = provider_width.max(display_width(&route_provider_display(
                &route.provider,
                &route.api_method,
            )));
        }
    }
    let mut lines = Vec::new();
    if search_rows > 0
        && let Some(line) = search_line
    {
        lines.push(line);
    }
    for row in start..start + visible {
        let entry = &picker.entries[picker.filtered[row]];
        let route = entry.active_option();
        let unavailable = route.is_some_and(|r| !r.available);
        let limited = route.is_some_and(|r| route_detail_is_limited(&r.detail));
        let style = if row == selected {
            Style::default().fg(rgb(255, 213, 128))
        } else {
            Style::default().fg(rgb(128, 203, 196))
        };
        let mut spans = vec![Span::styled(
            format!(
                "{} {}",
                picker_row_marker(row == selected, unavailable, limited),
                picker_entry_display_name(entry)
            ),
            style,
        )];
        if entry.is_current {
            spans.push(Span::styled(" current", dim));
        }
        if let Some(route) = route {
            let route_style = if row == selected { style } else { dim };
            let name_width = display_width(&picker_entry_display_name(entry))
                + if entry.is_current {
                    " current".len()
                } else {
                    0
                };
            spans.push(Span::raw(" ".repeat(model_width - name_width)));
            spans.push(Span::styled(
                format!(
                    "  {}",
                    pad_left_display(
                        &route_provider_display(&route.provider, &route.api_method),
                        provider_width,
                    )
                ),
                if row == selected && !picker.preview && picker.column == 1 {
                    route_style.bold().underlined()
                } else {
                    route_style
                },
            ));
            spans.push(Span::styled(" · ", dim));
            spans.push(Span::styled(
                route_method_display(route),
                if row == selected && !picker.preview && picker.column == 2 {
                    route_style.bold().underlined()
                } else {
                    route_style
                },
            ));
        }
        if row == start && start > 0 {
            spans.push(Span::styled(format!("  ↑{start}"), dim));
        }
        if row + 1 == start + visible && row + 1 < picker.filtered.len() {
            spans.push(Span::styled(
                format!("  +{} more", picker.filtered.len() - row - 1),
                dim,
            ));
        }
        lines.push(Line::from(spans));
    }
    if notice_rows > 0 {
        lines.push(Line::from(Span::styled(notice.unwrap().0, dim)));
    }
    if hint_rows > 0 {
        let filter = if !browsing && !picker.preview && !picker.filter.is_empty() {
            format!("Filter: {} · ", picker.filter)
        } else {
            String::new()
        };
        let navigation = if !picker.preview && picker.column > 0 {
            "↑↓ route · ←→ column · Enter select · Esc cancel"
        } else if browsing {
            "↑↓ scroll · type to search · Ctrl+P provider · Enter select · Esc"
        } else if picker.preview
            && picker
                .entries
                .iter()
                .any(|entry| matches!(entry.action, crate::tui::PickerAction::Model))
        {
            "↑↓ choose · Enter select · Ctrl+P provider · Esc cancel"
        } else {
            "↑↓ choose · Enter select · Esc cancel"
        };
        let shortcuts = hint
            .map(|hint| format!(" ·{}", hint.trim_start_matches(" keys:")))
            .unwrap_or_default();
        lines.push(Line::from(Span::styled(
            format!("{filter}{navigation}{shortcuts}"),
            dim,
        )));
    }
    lines
}

pub(super) fn draw_inline_interactive(frame: &mut Frame, app: &dyn TuiState, area: Rect) {
    let picker = match app.inline_interactive_state() {
        Some(p) => p,
        None => return,
    };

    let height = area.height as usize;
    let width = area.width as usize;
    if height <= 2 || width <= 2 {
        return;
    }
    if picker.is_agent_profile_sources() {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(rgb(85, 85, 110)));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(agent_profile_source_lines(
                picker,
                inner.height as usize,
                inner.width as usize,
            )),
            inner,
        );
        return;
    }

    let selected = picker.selected;
    let total = picker.entries.len();
    let filtered_count = picker.filtered.len();
    let col = picker.column;
    let is_preview = picker.preview;
    let is_account_picker = picker.uses_compact_navigation();
    let is_usage_picker = picker.kind == crate::tui::PickerKind::Usage;

    let col_focus_style = Style::default().fg(accent_color()).bold();
    let col_dim_style = Style::default().fg(dim_color());
    let marker_width = 3usize;
    const WIDTH_SCAN_LIMIT: usize = 200;

    let show_account_provider_badge =
        is_account_picker && account_picker_shows_provider_badge(picker);
    let mut max_provider_len = display_width(picker.secondary_label(is_preview));
    let mut max_via_len = display_width(picker.tertiary_label());
    let mut max_model_len = display_width(picker.primary_label());
    let mut max_account_title_len = display_width("ACCOUNT");
    let mut max_account_state_len = display_width("STATE");
    for &fi in picker.filtered.iter().take(WIDTH_SCAN_LIMIT) {
        let entry = &picker.entries[fi];
        let route = entry.active_option();
        max_model_len = max_model_len.max(display_width(&picker_entry_display_name(entry)));
        if let Some(r) = route {
            max_provider_len = max_provider_len.max(display_width(r.provider.as_str()));
            max_via_len = max_via_len.max(display_width(&route_method_display(r)));
        }
        if is_account_picker {
            let (title, _) = account_picker_entry_title(entry, show_account_provider_badge);
            max_account_title_len = max_account_title_len.max(display_width(title.as_str()));
            max_account_state_len = max_account_state_len
                .max(display_width(account_inline_interactive_state_label(entry)));
        }
    }
    max_provider_len = max_provider_len.max(8);
    max_via_len = max_via_len.max(3);

    let content_width = picker_render_width(picker, width.saturating_sub(2)).max(1);
    let outer_width = content_width.saturating_add(2).min(width);
    let horizontal_offset = if app.centered_mode() {
        area.width.saturating_sub(outer_width as u16) / 2
    } else {
        0
    };

    // Hotkey hint sits ABOVE the picker box (outside its border) so the
    // shortcuts are always visible without competing with the column headers.
    let keybind_hint = model_picker_top_hint(picker);
    let hint_rows: u16 = if keybind_hint.is_some() && area.height > 3 {
        1
    } else {
        0
    };
    if let Some(hint) = keybind_hint.filter(|_| hint_rows == 1) {
        // The hint lives outside the box, so it may use the full available
        // width rather than the (often narrow) intrinsic box width. This keeps
        // the favorites/default shortcuts visible even for short model lists.
        let hint_area = Rect {
            x: area.x,
            y: area.y,
            width: area.width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_display(hint, area.width.saturating_sub(1) as usize),
                Style::default().fg(rgb(120, 120, 150)).italic(),
            ))),
            hint_area,
        );
    }

    let render_area = Rect {
        x: area.x + horizontal_offset,
        y: area.y + hint_rows,
        width: outer_width as u16,
        height: area.height.saturating_sub(hint_rows),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(rgb(85, 85, 110)))
        .style(Style::default().bg(rgb(18, 18, 26)));
    frame.render_widget(block.clone(), render_area);

    let inner = block.inner(render_area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let height = inner.height as usize;
    let width = inner.width as usize;

    let mut provider_width = (max_provider_len + 1).max(8);
    let mut via_width = (max_via_len + 1).max(6);
    if !is_account_picker {
        let min_model_width = max_model_len.clamp(6, 16);
        let needed = marker_width + provider_width + via_width + min_model_width;
        if needed > width {
            let provider_floor = 8usize.min(provider_width);
            let via_floor = 6usize.min(via_width);
            let provider_reduction = needed
                .saturating_sub(width)
                .min(provider_width.saturating_sub(provider_floor));
            provider_width = provider_width.saturating_sub(provider_reduction);
            let still_needed = marker_width + provider_width + via_width + min_model_width;
            let via_reduction = still_needed
                .saturating_sub(width)
                .min(via_width.saturating_sub(via_floor));
            via_width = via_width.saturating_sub(via_reduction);
        }
    }
    let account_state_width = (max_account_state_len + 1).clamp(7, 10);
    let account_title_width = width.saturating_sub(marker_width + account_state_width);
    let model_width = width.saturating_sub(marker_width + provider_width + via_width);

    let (col_labels, col_logical) = picker.header_layout(is_preview);
    let col_widths: [usize; 3] = if is_account_picker {
        [account_title_width, account_state_width, 0]
    } else if is_preview {
        [provider_width, model_width, via_width]
    } else {
        [model_width, provider_width, via_width]
    };

    let mut header_spans: Vec<Span> = Vec::new();

    let first_label = col_labels[0];
    let first_w = marker_width + col_widths[0];
    let first_style = if col_logical[0] == col {
        col_focus_style
    } else {
        col_dim_style
    };
    header_spans.push(Span::styled(
        format!(" {:<w$}", first_label, w = first_w.saturating_sub(1)),
        first_style,
    ));

    let second_label = col_labels[1];
    let second_w = col_widths[1];
    let second_style = if col_logical[1] == col {
        col_focus_style
    } else {
        col_dim_style
    };
    header_spans.push(Span::styled(
        if is_preview {
            format!(" {:<w$}", second_label, w = second_w.saturating_sub(1))
        } else {
            format!("{:<w$}", second_label, w = second_w)
        },
        second_style,
    ));

    if !is_account_picker {
        let third_label = col_labels[2];
        let third_style = if col_logical[2] == col {
            col_focus_style
        } else {
            col_dim_style
        };
        header_spans.push(Span::styled(format!(" {}", third_label), third_style));
    }

    let mut meta_parts = String::new();
    if !picker.filter.is_empty() {
        meta_parts.push_str(&format!("  \"{}\"", picker.filter));
    }
    let count_str = if filtered_count == total {
        format!(" ({})", total)
    } else {
        format!(" ({}/{})", filtered_count, total)
    };
    meta_parts.push_str(&count_str);
    header_spans.push(Span::styled(meta_parts, Style::default().fg(dim_color())));

    if is_preview {
        header_spans.push(Span::styled(
            picker.preview_submit_hint(),
            Style::default().fg(rgb(60, 60, 80)).italic(),
        ));
    } else {
        header_spans.push(Span::styled(
            picker.active_submit_hint(),
            Style::default().fg(rgb(60, 60, 80)),
        ));
        if picker.shows_default_shortcut_hint() {
            header_spans.push(Span::styled(
                "  Ctrl-O=set default",
                Style::default().fg(rgb(60, 60, 80)).italic(),
            ));
        }
    }

    let row_base_width = if is_account_picker {
        marker_width + account_title_width + account_state_width
    } else {
        marker_width + provider_width + via_width + model_width
    };
    let detail_width = width.saturating_sub(row_base_width).saturating_sub(2);

    let selected_route_notice = picker
        .filtered
        .get(selected)
        .and_then(|entry_idx| picker.entries.get(*entry_idx))
        .and_then(|entry| selected_route_notice_text(picker, entry.active_option()));

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(header_spans));
    if let Some((notice, warning)) = selected_route_notice.as_ref() {
        let notice_width = width.saturating_sub(1);
        lines.push(Line::from(Span::styled(
            format!(" {}", truncate_display(notice.as_str(), notice_width)),
            if *warning {
                Style::default().fg(rgb(210, 150, 110)).italic()
            } else {
                Style::default().fg(dim_color()).italic()
            },
        )));
    }

    if picker.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no matches",
            Style::default().fg(dim_color()).italic(),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let list_header_lines = 1 + usize::from(selected_route_notice.is_some());
    let list_height = height.saturating_sub(list_header_lines);
    if list_height == 0 {
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let half = list_height / 2;
    let start = if selected <= half {
        0
    } else if selected + list_height - half > filtered_count {
        filtered_count.saturating_sub(list_height)
    } else {
        selected - half
    };
    let end = (start + list_height).min(filtered_count);

    for vi in start..end {
        let model_idx = picker.filtered[vi];
        let entry = &picker.entries[model_idx];
        let is_row_selected = vi == selected;
        let route = entry.active_option();
        let unavailable = route.map(|r| !r.available).unwrap_or(true);

        let limited = route
            .map(|r| r.available && route_detail_is_limited(&r.detail))
            .unwrap_or(false);
        let marker = picker_row_marker(is_row_selected, unavailable, limited);

        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::styled(
            format!(" {} ", marker),
            if unavailable {
                Style::default().fg(rgb(180, 120, 120)).bold()
            } else if is_row_selected {
                Style::default().fg(Color::White).bold()
            } else {
                Style::default().fg(dim_color())
            },
        ));
        let display_name = picker_entry_display_name(entry);
        let account_action_color = match &entry.action {
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Add { .. }) => {
                Some(rgb(140, 220, 170))
            }
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Replace {
                ..
            }) => Some(rgb(240, 200, 120)),
            crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::OpenCenter {
                ..
            }) => Some(rgb(150, 190, 255)),
            _ => None,
        };
        let primary_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 0 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else if let Some(color) = account_action_color {
            Style::default().fg(color).bold()
        } else if entry.is_current {
            Style::default().fg(accent_color())
        } else if entry.is_favorite {
            Style::default().fg(rgb(255, 160, 210)).bold()
        } else if entry.recommended {
            Style::default().fg(rgb(255, 220, 120))
        } else if entry.old {
            Style::default().fg(rgb(120, 120, 130))
        } else {
            Style::default().fg(rgb(200, 200, 220))
        };

        if is_account_picker {
            let (title_text, title_prefix_chars) =
                account_picker_entry_title(entry, show_account_provider_badge);
            let padded_title = pad_left_display(title_text.as_str(), account_title_width);
            let state_label = account_inline_interactive_state_label(entry);
            let state_display = format!(
                " {}",
                pad_left_display(state_label, account_state_width.saturating_sub(1))
            );
            let match_positions = if !picker.filter.is_empty() {
                fuzzy_match_positions(&picker.filter, &entry.name)
                    .into_iter()
                    .map(|p| p + title_prefix_chars)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let title_spans: Vec<Span> = if match_positions.is_empty() || unavailable {
                vec![Span::styled(padded_title, primary_style)]
            } else {
                let title_chars: Vec<char> = padded_title.chars().collect();
                let highlight_style = primary_style.underlined();
                let mut result = Vec::new();
                let mut run_start = 0;
                let mut is_match_run = !title_chars.is_empty() && match_positions.contains(&0);
                for ci in 1..=title_chars.len() {
                    let cur_is_match = ci < title_chars.len() && match_positions.contains(&ci);
                    if cur_is_match != is_match_run || ci == title_chars.len() {
                        let chunk: String = title_chars[run_start..ci].iter().collect();
                        result.push(Span::styled(
                            chunk,
                            if is_match_run {
                                highlight_style
                            } else {
                                primary_style
                            },
                        ));
                        run_start = ci;
                        is_match_run = cur_is_match;
                    }
                }
                result
            };

            let state_style = if unavailable {
                Style::default().fg(rgb(80, 80, 80))
            } else if is_row_selected {
                Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
            } else if entry.is_current {
                Style::default().fg(accent_color()).bold()
            } else if let Some(color) = account_action_color {
                Style::default().fg(color)
            } else {
                Style::default().fg(dim_color())
            };

            spans.extend(title_spans);
            spans.push(Span::styled(state_display, state_style));
            if let Some(route) = route
                && let Some(detail_text) = route_detail_display_text(&route.detail, unavailable)
                && detail_width > 0
            {
                spans.push(Span::styled(
                    format!("  {}", truncate_display(detail_text.as_str(), detail_width)),
                    if unavailable {
                        Style::default().fg(rgb(180, 120, 120)).italic()
                    } else {
                        Style::default().fg(dim_color())
                    },
                ));
            }

            lines.push(Line::from(spans));
            continue;
        }

        let padded_model = if is_preview {
            format!(
                " {}",
                pad_left_display(display_name.as_str(), model_width.saturating_sub(1))
            )
        } else {
            pad_left_display(display_name.as_str(), model_width)
        };

        let match_positions = if !picker.filter.is_empty() {
            let raw = fuzzy_match_positions(&picker.filter, display_name.as_str());
            if is_preview && !raw.is_empty() {
                // Account for the single leading space in the preview model column.
                raw.into_iter().map(|p| p + 1).collect()
            } else {
                raw
            }
        } else {
            Vec::new()
        };
        let model_spans: Vec<Span> = if match_positions.is_empty() || unavailable {
            vec![Span::styled(padded_model, primary_style)]
        } else {
            let model_chars: Vec<char> = padded_model.chars().collect();
            let highlight_style = primary_style.underlined();
            let mut result = Vec::new();
            let mut run_start = 0;
            let mut is_match_run = !model_chars.is_empty() && match_positions.contains(&0);
            for ci in 1..=model_chars.len() {
                let cur_is_match = ci < model_chars.len() && match_positions.contains(&ci);
                if cur_is_match != is_match_run || ci == model_chars.len() {
                    let chunk: String = model_chars[run_start..ci].iter().collect();
                    result.push(Span::styled(
                        chunk,
                        if is_match_run {
                            highlight_style
                        } else {
                            primary_style
                        },
                    ));
                    run_start = ci;
                    is_match_run = cur_is_match;
                }
            }
            result
        };

        let route_count = entry.option_count();
        let provider_raw = route
            .map(|r| route_provider_display(&r.provider, &r.api_method))
            .unwrap_or_else(|| "-".to_string());
        let provider_label = if col == 0 && route_count > 1 {
            format!("{} ({})", provider_raw, route_count)
        } else {
            provider_raw
        };
        let pw = provider_width.saturating_sub(1);
        let provider_display = format!(" {}", pad_left_display(provider_label.as_str(), pw));
        let provider_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 1 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else {
            Style::default().fg(rgb(140, 180, 255))
        };

        let via_raw = route
            .map(route_method_display)
            .unwrap_or_else(|| "-".to_string());
        let vw = via_width.saturating_sub(1);
        let via_display = format!(" {}", pad_left_display(via_raw.as_str(), vw));
        let via_style = if unavailable {
            Style::default().fg(rgb(80, 80, 80))
        } else if is_row_selected && col == 2 {
            Style::default().fg(Color::White).bg(rgb(60, 60, 80)).bold()
        } else if is_usage_picker {
            Style::default().fg(rgb(196, 170, 255))
        } else {
            Style::default().fg(rgb(220, 190, 120))
        };

        if is_preview && !is_account_picker {
            spans.push(Span::styled(provider_display, provider_style));
            spans.extend(model_spans);
            spans.push(Span::styled(via_display, via_style));
        } else {
            spans.extend(model_spans);
            spans.push(Span::styled(provider_display, provider_style));
            spans.push(Span::styled(via_display, via_style));
        }

        if let Some(route) = route
            && let Some(detail_text) = route_detail_display_text(&route.detail, unavailable)
            && detail_width > 0
        {
            spans.push(Span::styled(
                format!("  {}", truncate_display(detail_text.as_str(), detail_width)),
                if unavailable {
                    Style::default().fg(rgb(180, 120, 120)).italic()
                } else {
                    Style::default().fg(dim_color())
                },
            ));
        }

        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
#[path = "ui_inline_interactive_tests.rs"]
mod tests;
