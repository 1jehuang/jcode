#![allow(dead_code)]

use super::{
    binary_age, dim_color, header_animation_color, header_chrome_color, header_fade_color,
    header_fade_t, header_icon_color, header_name_color, header_session_color,
    is_running_stable_release, semver, shorten_model_name, TuiState,
};
use crate::auth::{AuthState, AuthStatus};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

fn format_model_name(short: &str) -> String {
    if short.contains('/') {
        return format!("OpenRouter: {}", short);
    }
    if short.contains("opus") {
        if short.contains("4.5") {
            return "Claude 4.5 Opus".to_string();
        }
        return "Claude Opus".to_string();
    }
    if short.contains("sonnet") {
        if short.contains("3.5") {
            return "Claude 3.5 Sonnet".to_string();
        }
        return "Claude Sonnet".to_string();
    }
    if short.contains("haiku") {
        return "Claude Haiku".to_string();
    }
    if short.starts_with("gpt") {
        return format_gpt_name(short);
    }
    short.to_string()
}

fn format_gpt_name(short: &str) -> String {
    let rest = short.trim_start_matches("gpt");
    if rest.is_empty() {
        return "GPT".to_string();
    }

    if let Some(idx) = rest.find("codex") {
        let version = &rest[..idx];
        if version.is_empty() {
            return "GPT Codex".to_string();
        }
        return format!("GPT-{} Codex", version);
    }

    format!("GPT-{}", rest)
}

fn pill_badge(label: &str, color: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled("  ", Style::default()),
        Span::styled("⟨ ", Style::default().fg(color)),
        Span::styled(label.to_string(), Style::default().fg(color)),
        Span::styled(" ⟩", Style::default().fg(color)),
    ]
}

fn multi_status_badge(items: &[(&str, Color)]) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(" ", Style::default()),
        Span::styled("⟨", Style::default().fg(dim_color())),
    ];

    for (i, (label, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("·", Style::default().fg(dim_color())));
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(*color)));
    }

    spans.push(Span::styled("⟩", Style::default().fg(dim_color())));
    spans
}

fn header_spans(icon: &str, session: &str, model: &str, elapsed: f32) -> Vec<Span<'static>> {
    let segments = [
        (format!("{} ", icon), header_icon_color(), 0.00),
        ("JCode ".to_string(), header_name_color(), 0.06),
        (
            format!("{} ", capitalize(session)),
            header_session_color(),
            0.12,
        ),
        ("· ".to_string(), dim_color(), 0.18),
        (model.to_string(), header_animation_color(elapsed), 0.12),
    ];

    let total_chars: usize = segments
        .iter()
        .map(|(text, _, _)| text.chars().count())
        .sum();
    let total = total_chars.max(1);
    let mut spans = Vec::with_capacity(total_chars);
    let mut idx = 0usize;

    for (text, target, offset) in segments {
        let fade = header_fade_t(elapsed, offset);
        let base = header_fade_color(target, elapsed, offset);
        for ch in text.chars() {
            let pos = if total > 1 {
                idx as f32 / (total - 1) as f32
            } else {
                0.0
            };
            let color = header_chrome_color(base, pos, elapsed, fade);
            spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
            idx += 1;
        }
    }

    spans
}

pub(super) fn build_auth_status_line(auth: &AuthStatus, max_width: usize) -> Line<'static> {
    fn dot_color(state: AuthState) -> Color {
        match state {
            AuthState::Available => rgb(100, 200, 100),
            AuthState::Expired => rgb(255, 200, 100),
            AuthState::NotConfigured => rgb(80, 80, 80),
        }
    }

    fn dot_char(state: AuthState) -> &'static str {
        match state {
            AuthState::Available => "●",
            AuthState::Expired => "◐",
            AuthState::NotConfigured => "○",
        }
    }

    fn rendered_width(entries: &[&str]) -> usize {
        if entries.is_empty() {
            return 0;
        }

        entries.iter().map(|label| label.len() + 3).sum::<usize>() + (entries.len() - 1)
    }

    fn provider_label(name: &str, state: AuthState, method: Option<&str>) -> String {
        match (state, method) {
            (AuthState::NotConfigured, _) => name.to_string(),
            (_, Some(method)) if !method.is_empty() => format!("{}({})", name, method),
            _ => name.to_string(),
        }
    }

    let anthropic_label = if auth.anthropic.has_oauth && auth.anthropic.has_api_key {
        provider_label("anthropic", auth.anthropic.state, Some("oauth+key"))
    } else if auth.anthropic.has_oauth {
        provider_label("anthropic", auth.anthropic.state, Some("oauth"))
    } else if auth.anthropic.has_api_key {
        provider_label("anthropic", auth.anthropic.state, Some("key"))
    } else {
        provider_label("anthropic", auth.anthropic.state, None)
    };

    let openai_label = if auth.openai_has_oauth && auth.openai_has_api_key {
        provider_label("openai", auth.openai, Some("oauth+key"))
    } else if auth.openai_has_oauth {
        provider_label("openai", auth.openai, Some("oauth"))
    } else if auth.openai_has_api_key {
        provider_label("openai", auth.openai, Some("key"))
    } else {
        provider_label("openai", auth.openai, None)
    };

    let full_specs: Vec<(String, AuthState)> = vec![
        (anthropic_label, auth.anthropic.state),
        ("openrouter".to_string(), auth.openrouter),
        (openai_label, auth.openai),
        (provider_label("cursor", auth.cursor, None), auth.cursor),
        (provider_label("copilot", auth.copilot, None), auth.copilot),
        (
            provider_label("antigravity", auth.antigravity, None),
            auth.antigravity,
        ),
    ];

    let compact_specs: Vec<(String, AuthState)> = vec![
        (
            provider_label("an", auth.anthropic.state, None),
            auth.anthropic.state,
        ),
        ("or".to_string(), auth.openrouter),
        (provider_label("oa", auth.openai, None), auth.openai),
        (provider_label("cu", auth.cursor, None), auth.cursor),
        (provider_label("cp", auth.copilot, None), auth.copilot),
        (
            provider_label("ag", auth.antigravity, None),
            auth.antigravity,
        ),
    ];

    let full: Vec<&str> = full_specs.iter().map(|(label, _)| label.as_str()).collect();
    let compact: Vec<&str> = compact_specs
        .iter()
        .map(|(label, _)| label.as_str())
        .collect();

    let provider_specs: Vec<&(String, AuthState)> = if rendered_width(&full) <= max_width {
        full_specs.iter().collect()
    } else if rendered_width(&compact) <= max_width {
        compact_specs.iter().collect()
    } else {
        compact_specs.iter().take(4).collect()
    };

    let mut spans = Vec::new();
    for (i, (label, state)) in provider_specs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" ", Style::default().fg(dim_color())));
        }

        spans.push(Span::styled(
            dot_char(*state),
            Style::default().fg(dot_color(*state)),
        ));
        spans.push(Span::styled(
            format!(" {} ", label),
            Style::default().fg(dim_color()),
        ));
    }

    Line::from(spans)
}

fn abbreviate_home(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.display().to_string();
        if path == home_str {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(&home_str) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

pub(super) fn build_persistent_header(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let model = app.provider_model();
    let session_name = app.session_display_name().unwrap_or_default();
    let server_name = app.server_display_name();
    let short_model = shorten_model_name(&model);
    let icon = crate::id::session_icon(&session_name);
    let nice_model = format_model_name(&short_model);
    let build_info = binary_age().unwrap_or_else(|| "unknown".to_string());
    let centered = app.centered_mode();
    let align = if centered {
        Alignment::Center
    } else {
        Alignment::Left
    };

    let mut lines: Vec<Line> = Vec::new();

    let is_canary = app.is_canary();
    let is_remote = app.is_remote_mode();
    let server_update = app.server_update_available() == Some(true);
    let client_update = app.client_update_available();
    let mut status_items: Vec<&str> = Vec::new();
    if app.is_replay() {
        status_items.push("replay");
    } else if is_remote {
        status_items.push("client");
    }
    if is_canary {
        status_items.push("dev");
    }
    if server_update {
        status_items.push("srv↑");
    }
    if client_update {
        status_items.push("cli↑");
    }
    if let Some(badge) = crate::perf::profile().tier.badge() {
        status_items.push(badge);
    }

    if !status_items.is_empty() {
        let badge_text = format!("⟨{}⟩", status_items.join("·"));
        lines.push(
            Line::from(Span::styled(badge_text, Style::default().fg(dim_color()))).alignment(align),
        );
    } else if centered {
        lines.push(Line::from(""));
    }

    if !session_name.is_empty() {
        let title_prefix = server_name
            .as_deref()
            .map(capitalize)
            .unwrap_or_else(|| "JCode".to_string());
        let server_icon = app.server_display_icon().unwrap_or_default();
        let icons = if server_icon.is_empty() {
            icon.to_string()
        } else {
            format!("{}{}", server_icon, icon)
        };
        let full_name = format!("{} {} {}", title_prefix, capitalize(&session_name), icons);
        lines.push(
            Line::from(Span::styled(
                full_name,
                Style::default().fg(header_name_color()),
            ))
            .alignment(align),
        );
    } else {
        let title_prefix = server_name
            .as_deref()
            .map(capitalize)
            .unwrap_or_else(|| "JCode".to_string());
        lines.push(
            Line::from(Span::styled(
                title_prefix,
                Style::default().fg(header_name_color()),
            ))
            .alignment(align),
        );
    }

    lines.push(
        Line::from(Span::styled(
            nice_model,
            Style::default().fg(header_session_color()),
        ))
        .alignment(align),
    );

    let w = width as usize;
    let version_text = if is_running_stable_release() {
        let tag = env!("JCODE_GIT_TAG");
        if tag.is_empty() || tag.contains('-') {
            let full = format!("{} · release · built {}", semver(), build_info);
            if full.chars().count() <= w {
                full
            } else {
                format!("{} · release", semver())
            }
        } else {
            let full = format!("{} · release {} · built {}", semver(), tag, build_info);
            if full.chars().count() <= w {
                full
            } else {
                format!("{} · {}", semver(), tag)
            }
        }
    } else {
        let full = format!("{} · built {}", semver(), build_info);
        if full.chars().count() <= w {
            full
        } else {
            semver().to_string()
        }
    };
    let version_line =
        Line::from(Span::styled(version_text, Style::default().fg(dim_color()))).alignment(align);
    lines.push(version_line);

    if let Some(dir) = app.working_dir() {
        let display_dir = abbreviate_home(&dir);
        lines.push(
            Line::from(Span::styled(display_dir, Style::default().fg(dim_color())))
                .alignment(align),
        );
    }

    lines
}

fn multi_status_badge_no_leading_space(items: &[(&str, Color)]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::styled("⟨", Style::default().fg(dim_color()))];

    for (i, (label, color)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("·", Style::default().fg(dim_color())));
        }
        spans.push(Span::styled(label.to_string(), Style::default().fg(*color)));
    }

    spans.push(Span::styled("⟩", Style::default().fg(dim_color())));
    spans
}
