use super::box_utils::render_rounded_box;
use super::changelog::get_unseen_changelog_entries;
use super::{
    TuiState, binary_age, dim_color, header_name_color, header_session_color,
    is_running_stable_release, semver, shorten_model_name,
};
use crate::auth::{AuthState, AuthStatus};
use crate::tui::color_support::rgb;
use crate::tui::connection_type_icon;
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn semver_core() -> String {
    semver()
        .split('-')
        .next()
        .unwrap_or_else(semver)
        .to_string()
}

#[cfg(test)]
fn semver_minor() -> String {
    let core = semver_core();
    let parts: Vec<&str> = core.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        core
    }
}

#[cfg(test)]
fn version_display_candidates() -> Vec<String> {
    let full = format!("jcode {}", semver());
    let core = format!("jcode {}", semver_core());
    let minor = format!("jcode {}", semver_minor());
    let shortest = semver_minor();
    vec![full, core, minor, shortest]
}

#[allow(dead_code)]
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
    if trimmed_model != model
        || !candidates
            .iter()
            .any(|candidate| candidate == trimmed_model)
    {
        candidates.push(trimmed_model.to_string());
    }

    candidates
}

#[allow(dead_code)]
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

pub(super) fn build_persistent_header(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let model = app.provider_model();
    let session_name = app.session_display_name().unwrap_or_default();
    let server_name = app.server_display_name();
    let short_model = shorten_model_name(&model);
    let icon = connection_type_icon(app.connection_type().as_deref())
        .unwrap_or_else(|| crate::id::session_icon(&session_name));
    let nice_model = format_model_name(&short_model);
    let build_info = binary_age().unwrap_or_else(|| "unknown".to_string());
    let align = Alignment::Center;
    let mut lines: Vec<Line> = Vec::new();
    let w = width as usize;

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
    } else {
        lines.push(Line::from(""));
    }

    if let Some(server_name) = server_name.as_deref() {
        let server_icon = app.server_display_icon().unwrap_or_default();
        let server_text = if server_icon.is_empty() {
            format!("server: {}", capitalize(server_name))
        } else {
            format!("server: {} {}", capitalize(server_name), server_icon)
        };
        lines.push(
            Line::from(Span::styled(
                server_text,
                Style::default().fg(header_name_color()),
            ))
            .alignment(align),
        );
    }

    if !session_name.is_empty() {
        let client_text = format!("client: {} {}", capitalize(&session_name), icon);
        lines.push(
            Line::from(Span::styled(
                client_text,
                Style::default().fg(header_name_color()),
            ))
            .alignment(align),
        );
    } else if server_name.is_none() {
        lines.push(
            Line::from(Span::styled(
                "JCode".to_string(),
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
    lines.push(
        Line::from(Span::styled(version_text, Style::default().fg(dim_color()))).alignment(align),
    );

    if let Some(dir) = app.working_dir() {
        let display_dir = abbreviate_home(&dir);
        lines.push(
            Line::from(Span::styled(display_dir, Style::default().fg(dim_color())))
                .alignment(align),
        );
    }

    lines
}

pub(crate) fn build_header_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let align = ratatui::layout::Alignment::Center;
    let model = app.provider_model();
    let provider_name = app.provider_name();
    let upstream = app.upstream_provider();
    let auth = app.auth_status();
    let w = width as usize;
    let model = model.trim().to_string();
    let provider_label = {
        let trimmed = provider_name.trim();
        if trimmed.is_empty() {
            String::new()
        } else {
            let name = trimmed.to_lowercase();
            let auth_tag = match name.as_str() {
                "anthropic" => {
                    if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                        "api-key"
                    } else if auth.anthropic.has_oauth {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "openai" => {
                    if auth.openai_has_api_key {
                        "api-key"
                    } else if auth.openai_has_oauth {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "copilot" => {
                    if auth.copilot_has_api_token {
                        "oauth"
                    } else {
                        ""
                    }
                }
                "openrouter" => "api-key",
                _ => "",
            };
            if auth_tag.is_empty() {
                name
            } else {
                format!("{}:{}", auth_tag, name)
            }
        }
    };

    let suppress_placeholder_detail = provider_label.is_empty()
        && upstream.is_none()
        && matches!(model.as_str(), "" | "connecting to server…" | "connected");

    let model_info = if suppress_placeholder_detail || model.is_empty() {
        String::new()
    } else if let Some(ref provider) = upstream {
        if provider_label.is_empty() {
            let full = format!("{} via {} · /model to switch", model, provider);
            if full.chars().count() <= w {
                full
            } else {
                format!("{} via {}", model, provider)
            }
        } else {
            let full = format!(
                "({}) {} via {} · /model to switch",
                provider_label, model, provider
            );
            if full.chars().count() <= w {
                full
            } else {
                let short = format!("({}) {} via {}", provider_label, model, provider);
                if short.chars().count() <= w {
                    short
                } else {
                    format!("({}) {}", provider_label, model)
                }
            }
        }
    } else if provider_label.is_empty() {
        let full = format!("{} · /model to switch", model);
        if full.chars().count() <= w {
            full
        } else {
            model.clone()
        }
    } else {
        let full = format!("({}) {} · /model to switch", provider_label, model);
        if full.chars().count() <= w {
            full
        } else {
            format!("({}) {}", provider_label, model)
        }
    };
    if !model_info.is_empty() {
        lines.push(
            Line::from(Span::styled(model_info, Style::default().fg(dim_color()))).alignment(align),
        );
    }

    let auth_line = build_auth_status_line(&auth, w);
    if !auth_line.spans.is_empty() {
        lines.push(auth_line.alignment(align));
    }

    if let Some(goal_badge) = crate::goal::header_badge(
        app.working_dir().as_deref().map(std::path::Path::new),
        app.side_panel(),
    ) {
        lines.push(
            Line::from(Span::styled(
                goal_badge,
                Style::default().fg(rgb(170, 200, 120)),
            ))
            .alignment(align),
        );
    }

    let new_entries = get_unseen_changelog_entries();
    if !new_entries.is_empty() && w > 20 {
        const MAX_LINES: usize = 8;
        let available_width = w.saturating_sub(2);
        let display_count = new_entries.len().min(MAX_LINES);
        let has_more = new_entries.len() > MAX_LINES;

        let mut content: Vec<Line> = Vec::new();
        for entry in new_entries.iter().take(display_count) {
            content.push(
                Line::from(Span::styled(
                    format!("• {}", entry),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
        }
        if has_more {
            content.push(
                Line::from(Span::styled(
                    format!(
                        "  …{} more · /changelog to see all",
                        new_entries.len() - MAX_LINES
                    ),
                    Style::default().fg(dim_color()),
                ))
                .alignment(align),
            );
        }

        let boxed = render_rounded_box(
            "Updates",
            content,
            available_width,
            Style::default().fg(dim_color()),
        );
        for line in boxed {
            lines.push(line.alignment(align));
        }
    }

    let mcps = app.mcp_servers();
    let mcp_text = if mcps.is_empty() {
        "mcp: (none)".to_string()
    } else {
        let full_parts: Vec<String> = mcps
            .iter()
            .map(|(name, count)| {
                if *count > 0 {
                    format!("{} ({} tools)", name, count)
                } else {
                    format!("{} (...)", name)
                }
            })
            .collect();
        let full = format!("mcp: {}", full_parts.join(", "));
        if full.chars().count() <= w {
            full
        } else {
            let short_parts: Vec<String> = mcps
                .iter()
                .map(|(name, count)| {
                    if *count > 0 {
                        format!("{}({})", name, count)
                    } else {
                        format!("{}(…)", name)
                    }
                })
                .collect();
            let short = format!("mcp: {}", short_parts.join(" "));
            if short.chars().count() <= w {
                short
            } else {
                format!("mcp: {} servers", mcps.len())
            }
        }
    };
    lines.push(
        Line::from(Span::styled(mcp_text, Style::default().fg(dim_color()))).alignment(align),
    );

    let skills = app.available_skills();
    if !skills.is_empty() {
        let full = format!(
            "skills: {}",
            skills
                .iter()
                .map(|s| format!("/{}", s))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let skills_text = if full.chars().count() <= w {
            full
        } else {
            format!("skills: {} loaded", skills.len())
        };
        lines.push(
            Line::from(Span::styled(skills_text, Style::default().fg(dim_color())))
                .alignment(align),
        );
    }

    let client_count = app.connected_clients().unwrap_or(0);
    let session_count = app.server_sessions().len();
    if client_count > 0 || session_count > 1 {
        let mut parts = Vec::new();
        if client_count > 0 {
            parts.push(format!(
                "{} client{}",
                client_count,
                if client_count == 1 { "" } else { "s" }
            ));
        }
        if session_count > 1 {
            parts.push(format!("{} sessions", session_count));
        }
        lines.push(
            Line::from(Span::styled(
                format!("server: {}", parts.join(", ")),
                Style::default().fg(dim_color()),
            ))
            .alignment(align),
        );
    }

    lines.push(Line::from(""));
    lines
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
    fn build_header_lines_omits_placeholder_provider_label_when_unknown() {
        let mut app = crate::tui::app::App::new_for_remote(None);
        app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

        let lines = build_header_lines(&app, 80);
        let rendered = lines
            .first()
            .expect("header line")
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(rendered.contains("loading session…"));
        assert!(!rendered.contains("(unknown)"));
        assert!(!rendered.contains("(remote)"));
    }

    #[test]
    fn build_header_lines_hides_secondary_placeholder_during_brief_connecting_phase() {
        let app = crate::tui::app::App::new_for_remote(None);

        let lines = build_header_lines(&app, 80);
        let rendered = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect::<String>();

        assert!(
            !rendered.contains("connecting to server…"),
            "brief connecting placeholder should not render the secondary detail line"
        );
        assert!(!rendered.contains("(remote)"));
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
