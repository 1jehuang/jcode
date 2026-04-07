#![allow(dead_code)]

use super::{
    TuiState, dim_color, header_animation_color, header_chrome_color, header_fade_color,
    header_fade_t, header_icon_color, header_name_color, header_session_color, semver,
    shorten_model_name,
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

    let gemini_label = if auth.gemini != AuthState::NotConfigured {
        provider_label("gemini", auth.gemini, Some("oauth"))
    } else {
        provider_label("gemini", auth.gemini, None)
    };

    let gemini_compact_label = if auth.gemini != AuthState::NotConfigured {
        provider_label("ge", auth.gemini, Some("oauth"))
    } else {
        provider_label("ge", auth.gemini, None)
    };

    let full_specs: Vec<(String, AuthState)> = vec![
        (anthropic_label, auth.anthropic.state),
        ("openrouter".to_string(), auth.openrouter),
        (openai_label, auth.openai),
        (provider_label("cursor", auth.cursor, None), auth.cursor),
        (provider_label("copilot", auth.copilot, None), auth.copilot),
        (gemini_label, auth.gemini),
        (
            provider_label("antigravity", auth.antigravity, None),
            auth.antigravity,
        ),
    ]
    .into_iter()
    .filter(|(_, state)| *state != AuthState::NotConfigured)
    .collect();

    let compact_specs: Vec<(String, AuthState)> = vec![
        (
            provider_label("an", auth.anthropic.state, None),
            auth.anthropic.state,
        ),
        ("or".to_string(), auth.openrouter),
        (provider_label("oa", auth.openai, None), auth.openai),
        (provider_label("cu", auth.cursor, None), auth.cursor),
        (provider_label("cp", auth.copilot, None), auth.copilot),
        (gemini_compact_label, auth.gemini),
        (
            provider_label("ag", auth.antigravity, None),
            auth.antigravity,
        ),
    ]
    .into_iter()
    .filter(|(_, state)| *state != AuthState::NotConfigured)
    .collect();

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

fn truncate_to_width(text: &str, width: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }

    let mut truncated = text
        .chars()
        .take(width.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

fn choose_header_candidate(width: usize, candidates: Vec<String>) -> String {
    let mut last_non_empty = String::new();
    for candidate in candidates
        .into_iter()
        .filter(|candidate| !candidate.trim().is_empty())
    {
        if candidate.chars().count() <= width {
            return candidate;
        }
        last_non_empty = candidate;
    }

    truncate_to_width(&last_non_empty, width)
}

fn path_display_candidates(path: &str) -> Vec<String> {
    let display = abbreviate_home(path);
    let mut candidates = vec![display.clone()];

    let parts: Vec<&str> = display.split('/').filter(|part| !part.is_empty()).collect();
    if parts.len() >= 2 {
        candidates.push(format!(
            "…/{}/{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        ));
    }
    if let Some(last) = parts.last() {
        candidates.push((*last).to_string());
    }

    candidates.dedup();
    candidates
}

fn semver_core() -> String {
    semver()
        .split('-')
        .next()
        .unwrap_or_else(semver)
        .to_string()
}

fn semver_minor() -> String {
    let core = semver_core();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        core
    }
}

fn version_display_candidates() -> Vec<String> {
    let full = format!("jcode {}", semver());
    let core = format!("jcode {}", semver_core());
    let minor = format!("jcode {}", semver_minor());
    let shortest = semver_minor();
    vec![full, core, minor, shortest]
}

fn provider_model_display_candidates(provider_name: &str, model: &str) -> Vec<String> {
    let trimmed_model = model.trim();
    if trimmed_model.is_empty() {
        return Vec::new();
    }

    let short_model = shorten_model_name(trimmed_model);
    let nice_model = format_model_name(&short_model);
    let provider = provider_name.trim().to_lowercase();
    let mut candidates = Vec::new();

    if !provider.is_empty() {
        candidates.push(format!("{} · {}", provider, nice_model));
        if short_model != nice_model {
            candidates.push(format!("{} · {}", provider, short_model));
        }
    }

    candidates.push(nice_model.clone());
    if short_model != nice_model {
        candidates.push(short_model);
    }
    if trimmed_model != model {
        candidates.push(trimmed_model.to_string());
    } else if !candidates
        .iter()
        .any(|candidate| candidate == trimmed_model)
    {
        candidates.push(trimmed_model.to_string());
    }

    candidates
}

fn configured_auth_count(auth: &AuthStatus) -> usize {
    [
        auth.jcode,
        auth.anthropic.state,
        auth.openrouter,
        auth.azure,
        auth.openai,
        auth.cursor,
        auth.copilot,
        auth.gemini,
        auth.antigravity,
        auth.google,
    ]
    .into_iter()
    .filter(|state| *state != AuthState::NotConfigured)
    .count()
}

fn memory_summary_label(memory_info: Option<&crate::tui::info_widget::MemoryInfo>) -> String {
    match memory_info {
        Some(info) => format!("memories {}", info.total_count),
        None => "memories off".to_string(),
    }
}

fn summary_display_candidates(
    memory_label: &str,
    skills_count: usize,
    mcp_count: usize,
    auth_count: usize,
) -> Vec<String> {
    let memories_short = memory_label.replacen("memories", "mem", 1);
    let skills_full = format!("skills {}", skills_count);
    let skills_short = format!("sk {}", skills_count);
    let mcp_full = format!("mcp {}", mcp_count);
    let auths_full = format!("auths {}", auth_count);

    vec![
        format!(
            "{} · {} · {} · {}",
            memory_label, skills_full, mcp_full, auths_full
        ),
        format!(
            "{} · {} · {} · {}",
            memories_short, skills_full, mcp_full, auths_full
        ),
        format!(
            "{} · {} · {} · {}",
            memories_short, skills_short, mcp_full, auths_full
        ),
        format!(
            "{} · {} · {} · {}",
            memories_short,
            skills_short,
            mcp_full,
            format!("auth {}", auth_count)
        ),
        format!(
            "{} · {} · {} · {}a",
            memories_short, skills_short, mcp_count, auth_count
        ),
        format!(
            "{} · {}s · {}m · {}a",
            memories_short, skills_count, mcp_count, auth_count
        ),
    ]
}

pub(super) fn build_persistent_header(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let align = Alignment::Center;
    let mut lines: Vec<Line> = Vec::new();
    let w = width as usize;

    if let Some(dir) = app.working_dir() {
        let display_dir = choose_header_candidate(w, path_display_candidates(&dir));
        lines.push(
            Line::from(Span::styled(
                display_dir,
                Style::default().fg(header_name_color()),
            ))
            .alignment(align),
        );
    }

    let version_text = choose_header_candidate(w, version_display_candidates());
    lines.push(
        Line::from(Span::styled(version_text, Style::default().fg(dim_color()))).alignment(align),
    );

    let provider_model_text = choose_header_candidate(
        w,
        provider_model_display_candidates(&app.provider_name(), &app.provider_model()),
    );
    if !provider_model_text.is_empty() {
        lines.push(
            Line::from(Span::styled(
                provider_model_text,
                Style::default().fg(header_session_color()),
            ))
            .alignment(align),
        );
    }

    lines
}

pub(crate) fn build_header_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let align = ratatui::layout::Alignment::Center;
    let auth = app.auth_status();
    let w = width as usize;
    let info = app.info_widget_data();
    let memory_label = memory_summary_label(info.memory_info.as_ref());
    let summary_text = choose_header_candidate(
        w,
        summary_display_candidates(
            &memory_label,
            app.available_skills().len(),
            app.mcp_servers().len(),
            configured_auth_count(&auth),
        ),
    );

    if summary_text.is_empty() {
        Vec::new()
    } else {
        vec![
            Line::from(Span::styled(summary_text, Style::default().fg(dim_color())))
                .alignment(align),
        ]
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthState, AuthStatus, ProviderAuth};
    use crate::message::Message;
    use crate::provider::{EventStream, Provider};
    use crate::tool::Registry;
    use anyhow::Result;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::OnceLock;

    struct MockProvider;

    #[async_trait]
    impl Provider for MockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "mock"
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(MockProvider)
        }
    }

    fn ensure_test_jcode_home_if_unset() {
        static TEST_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();

        if std::env::var_os("JCODE_HOME").is_some() {
            return;
        }

        let path = TEST_HOME.get_or_init(|| {
            let path = std::env::temp_dir().join(format!("jcode-test-home-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&path);
            path
        });
        crate::env::set_var("JCODE_HOME", path);
    }

    fn create_test_app() -> crate::tui::app::App {
        ensure_test_jcode_home_if_unset();

        let provider: Arc<dyn Provider> = Arc::new(MockProvider);
        let rt = tokio::runtime::Runtime::new().expect("test runtime");
        let registry = rt.block_on(Registry::new(provider.clone()));
        crate::tui::app::App::new(provider, registry)
    }

    #[test]
    fn left_aligned_mode_keeps_persistent_header_centered() {
        let mut app = create_test_app();
        app.set_centered(false);

        let lines = build_persistent_header(&app, 80);
        let non_empty: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| !line.spans.iter().all(|span| span.content.trim().is_empty()))
            .collect();

        assert!(!non_empty.is_empty(), "expected persistent header lines");
        assert!(
            non_empty
                .iter()
                .all(|line| line.alignment == Some(Alignment::Center)),
            "persistent header should remain centered in left-aligned mode: {non_empty:?}"
        );
    }

    #[test]
    fn left_aligned_mode_keeps_secondary_header_centered() {
        let mut app = create_test_app();
        app.set_centered(false);

        let lines = build_header_lines(&app, 80);
        let non_empty: Vec<&Line<'_>> = lines
            .iter()
            .filter(|line| !line.spans.iter().all(|span| span.content.trim().is_empty()))
            .collect();

        assert!(!non_empty.is_empty(), "expected header detail lines");
        assert!(
            non_empty
                .iter()
                .all(|line| line.alignment == Some(Alignment::Center)),
            "header detail lines should remain centered in left-aligned mode: {non_empty:?}"
        );
    }

    #[test]
    fn version_display_candidates_compact_for_narrow_width() {
        let rendered = choose_header_candidate(8, version_display_candidates());
        assert_eq!(rendered, "v0.9");
    }

    #[test]
    fn summary_display_candidates_compact_before_dropping_counts() {
        let rendered =
            choose_header_candidate(28, summary_display_candidates("memories 87", 12, 3, 6));
        assert!(rendered.contains("87"), "rendered: {rendered}");
        assert!(rendered.contains("12"), "rendered: {rendered}");
        assert!(rendered.contains("3"), "rendered: {rendered}");
        assert!(rendered.contains("6"), "rendered: {rendered}");
        assert!(!rendered.is_empty());
    }

    #[test]
    fn configured_auth_count_includes_non_model_auth_surfaces() {
        let auth = AuthStatus {
            jcode: AuthState::Available,
            anthropic: ProviderAuth {
                state: AuthState::Expired,
                has_oauth: true,
                has_api_key: false,
            },
            azure: AuthState::Available,
            google: AuthState::Available,
            ..AuthStatus::default()
        };

        assert_eq!(configured_auth_count(&auth), 4);
    }

    #[test]
    fn build_persistent_header_prefers_configured_model_during_remote_connect() {
        let _guard = crate::storage::lock_test_env();
        let prev_model = std::env::var_os("JCODE_MODEL");
        let prev_provider = std::env::var_os("JCODE_PROVIDER");
        crate::env::set_var("JCODE_MODEL", "gpt-5.4");
        crate::env::set_var("JCODE_PROVIDER", "openai");

        let app = crate::tui::app::App::new_for_remote(None);
        let lines = build_persistent_header(&app, 80);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("GPT-5.4"));
        assert!(!rendered.contains("connecting to server…"));

        if let Some(prev_model) = prev_model {
            crate::env::set_var("JCODE_MODEL", prev_model);
        } else {
            crate::env::remove_var("JCODE_MODEL");
        }
        if let Some(prev_provider) = prev_provider {
            crate::env::set_var("JCODE_PROVIDER", prev_provider);
        } else {
            crate::env::remove_var("JCODE_PROVIDER");
        }
    }

    #[test]
    fn build_header_lines_renders_summary_counts() {
        let app = create_test_app();
        let lines = build_header_lines(&app, 120);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("memories"), "rendered: {rendered}");
        assert!(rendered.contains("skills"), "rendered: {rendered}");
        assert!(rendered.contains("mcp"), "rendered: {rendered}");
        assert!(rendered.contains("auths"), "rendered: {rendered}");
    }

    #[test]
    fn auth_status_line_hides_not_configured_providers() {
        let auth = AuthStatus {
            anthropic: ProviderAuth {
                state: AuthState::Expired,
                has_oauth: true,
                has_api_key: false,
            },
            openai: AuthState::Available,
            openai_has_oauth: false,
            openai_has_api_key: true,
            ..AuthStatus::default()
        };

        let line = build_auth_status_line(&auth, 120);
        let rendered = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(
            rendered.contains("anthropic(oauth)"),
            "rendered: {rendered}"
        );
        assert!(rendered.contains("openai(key)"), "rendered: {rendered}");
        assert!(!rendered.contains("openrouter"), "rendered: {rendered}");
        assert!(!rendered.contains("copilot"), "rendered: {rendered}");
        assert!(!rendered.contains("cursor"), "rendered: {rendered}");
    }

    #[test]
    fn auth_status_line_is_empty_when_nothing_was_attempted() {
        let line = build_auth_status_line(&AuthStatus::default(), 120);
        assert!(line.spans.is_empty(), "line should be empty: {line:?}");
    }
}
