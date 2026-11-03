use super::box_utils::render_rounded_box;
use super::changelog::get_unseen_changelog_entries;
use super::{
    TuiState, dim_color, header_name_color, is_running_stable_release, semver, shorten_model_name,
};
use crate::auth::{AuthState, AuthStatus};
use crate::tui::color_support::rgb;
use crate::tui::connection_type_icon;
use ratatui::prelude::*;
#[path = "ui_header/account_router.rs"]
mod account_router;

/// UI chrome is single-line. Budget the same trailing cell as the transcript
/// preparer so even an exactly-full header cannot acquire a continuation row.
fn fit_header_lines(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let budget = usize::from(width.saturating_sub(1));
    lines
        .into_iter()
        .map(|mut line| {
            for span in &mut line.spans {
                if span.content.chars().any(char::is_control) {
                    span.content = span
                        .content
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect::<String>()
                        .into();
                }
            }
            let mut fitted = jcode_tui_render::truncate_line_with_ellipsis_to_width(&line, budget);
            fitted.style = line.style;
            fitted.alignment = line.alignment;
            fitted
        })
        .collect()
}

fn product_header_line(is_canary: bool, status_items: &[&str]) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "jcode",
        Style::default().fg(header_name_color()).bold(),
    )];
    if is_canary {
        spans.push(Span::styled(" self-dev", Style::default().fg(dim_color())));
    }
    if !status_items.is_empty() {
        spans.push(Span::styled(
            format!(" · {}", status_items.join(" · ")),
            Style::default().fg(dim_color()),
        ));
    }
    Line::from(spans).alignment(Alignment::Left)
}

fn client_header_text(name: &str, icon: &str, connection: Option<&str>) -> String {
    match connection {
        Some(conn) => format!("client: {} {} {}", capitalize(name), icon, conn),
        None => format!("client: {} {}", capitalize(name), icon),
    }
}

fn model_header_span(model: String) -> Span<'static> {
    Span::styled(model, Style::default().fg(rgb(255, 150, 200)).bold())
}

fn model_is_placeholder(model: &str) -> bool {
    let trimmed = model.trim();
    trimmed.is_empty()
        || trimmed == "connected"
        || trimmed.ends_with('…')
        || trimmed.starts_with("connecting")
}

/// Shared live/preview model chrome. Measure terminal cells, not scalar values.
fn model_header_line(
    nice_model: &str,
    provider_label: &str,
    upstream: Option<&str>,
    width: usize,
    placeholder: bool,
) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let mut spans = Vec::new();
    let mut used = nice_model.width();
    let budget = width.saturating_sub(4);
    if !placeholder && !nice_model.is_empty() {
        let hint = "/model to switch · ";
        if used + hint.width() <= budget {
            used += hint.width();
            spans.push(Span::styled(hint, Style::default().fg(dim_color())));
        }
    }
    if !provider_label.is_empty() && !placeholder {
        let prefix = format!("{provider_label} · ");
        if used + prefix.width() <= budget {
            used += prefix.width();
            spans.push(Span::styled(prefix, Style::default().fg(dim_color())));
        }
    }
    spans.push(model_header_span(nice_model.to_owned()));
    if let Some(upstream) = upstream.filter(|_| !placeholder) {
        let suffix = format!(" via {upstream}");
        if used + suffix.width() <= budget {
            spans.push(Span::styled(suffix, Style::default().fg(dim_color())));
        }
    }
    Line::from(spans).alignment(Alignment::Left)
}

fn standalone_version_line() -> Line<'static> {
    let text = if is_running_stable_release() {
        let tag = jcode_build_meta::git_tag();
        if tag.is_empty() || tag.contains('-') {
            format!("{} · release", semver())
        } else {
            format!("{} · release {}", semver(), tag)
        }
    } else {
        semver().to_string()
    };
    Line::from(Span::styled(text, Style::default().fg(dim_color()))).alignment(Alignment::Left)
}

/// The persisted subset of the real header. Credentials, transport and binary
/// versions are deliberately omitted when the selected session does not own them.
pub(crate) fn build_preview_header(
    session: &crate::tui::session_picker::SessionInfo,
    width: u16,
) -> Vec<Line<'static>> {
    let mut lines = vec![product_header_line(session.is_canary, &[])];
    if let Some(server) = session.server_name.as_deref() {
        lines.push(Line::from(Span::styled(
            format!(
                "server: {}{}",
                capitalize(server),
                session
                    .server_icon
                    .as_deref()
                    .map(|icon| format!(" {icon}"))
                    .unwrap_or_default()
            ),
            Style::default().fg(dim_color()),
        )));
    }
    if !session.short_name.is_empty() {
        let icon = crate::id::session_icon(&session.short_name);
        lines.push(Line::from(Span::styled(
            client_header_text(&session.short_name, icon, None),
            Style::default().fg(dim_color()),
        )));
    }
    if let Some(model) = session.model.as_deref().filter(|model| !model.is_empty()) {
        let nice_model =
            header_model_display_name(model, session.provider_key.as_deref().unwrap_or(""));
        lines.push(model_header_line(
            &nice_model,
            session.provider_key.as_deref().unwrap_or(""),
            None,
            width as usize,
            model_is_placeholder(model),
        ));
    }
    // Resuming a local transcript runs in this client, so its version belongs
    // to the preview too. Historical daemon versions are not persisted.
    if session.server_name.is_none() {
        lines.push(standalone_version_line());
    }
    fit_header_lines(lines, width)
}
#[cfg(test)]
use std::sync::OnceLock;

#[cfg(test)]
fn unseen_changelog_entries_override() -> &'static std::sync::Mutex<Option<Vec<String>>> {
    static OVERRIDE: OnceLock<std::sync::Mutex<Option<Vec<String>>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| std::sync::Mutex::new(None))
}

fn unseen_changelog_entries() -> Vec<String> {
    #[cfg(test)]
    {
        if let Ok(guard) = unseen_changelog_entries_override().lock()
            && let Some(entries) = guard.clone()
        {
            return entries;
        }
    }
    get_unseen_changelog_entries().clone()
}

#[cfg(test)]
pub(crate) fn set_unseen_changelog_entries_override_for_tests(entries: Option<Vec<String>>) {
    let mut guard = unseen_changelog_entries_override()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = entries;
}

/// Keep an override in place only while its owning render test is in scope.
/// Callers must hold the shared render-state test lock before creating this guard.
#[cfg(test)]
pub(crate) struct ChangelogEntriesOverrideGuard;

#[cfg(test)]
pub(crate) fn scoped_unseen_changelog_entries_override_for_tests(
    entries: Vec<String>,
) -> ChangelogEntriesOverrideGuard {
    set_unseen_changelog_entries_override_for_tests(Some(entries));
    ChangelogEntriesOverrideGuard
}

#[cfg(test)]
impl Drop for ChangelogEntriesOverrideGuard {
    fn drop(&mut self) {
        set_unseen_changelog_entries_override_for_tests(None);
    }
}

pub(super) fn account_router_generation() -> u64 {
    if crate::tui::is_ssh_remote() {
        return 0;
    }
    account_router::generation()
}

pub(crate) fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
    }
}

/// Compact form of a full build version string: `v0.25.19-dev (abc1234, dirty)`
/// becomes `v0.25.19-dev`. Used for the per-line server/client version labels.
fn compact_version_label(version: &str) -> String {
    let trimmed = version.trim();
    match trimmed.split_once(" (") {
        Some((head, _)) => head.trim().to_string(),
        None => trimmed.to_string(),
    }
}

/// Version label for a `server:`/`client:` header line. Normally compact
/// (semver only); keeps the git-hash suffix when the two sides share a semver
/// but differ by build, so the mismatch is still visible at a glance.
fn header_version_label(version: &str, include_hash: bool) -> String {
    if include_hash {
        version.trim().to_string()
    } else {
        compact_version_label(version)
    }
}

fn format_model_name(short: &str, provider_name: &str) -> String {
    if short.contains('/') {
        // Slashed model ids (e.g. `nvidia/nemotron-...`) are served by the
        // OpenRouter slot, which also fronts direct OpenAI-compatible profiles
        // such as NVIDIA NIM or DeepSeek. Label the line with the active
        // provider's display name instead of hard-coding "OpenRouter" so the
        // header matches the profile the user actually selected.
        let label = {
            let trimmed = provider_name.trim();
            if trimmed.is_empty() {
                "OpenRouter".to_string()
            } else {
                trimmed.to_string()
            }
        };
        return format!("{}: {}", label, short);
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
        // Only the numeric GPT families (gpt-4o, gpt-5.2-codex, ...) have a
        // curated form. Other gpt-prefixed ids (gpt-oss-120b) fall through to
        // the generic prettifier instead of producing "GPT-oss120b".
        let rest = short.trim_start_matches("gpt");
        if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit()) {
            return format_gpt_name(short);
        }
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

/// Generic fallback for model ids with no curated pretty name: title-case the
/// hyphen/underscore segments (`claude-fable-5` -> `Claude Fable 5`). Date or
/// snapshot suffixes (6+ digit runs) are dropped, vowel-less short segments are
/// treated as acronyms (`glm` -> `GLM`), and parameter sizes are uppercased
/// (`70b` -> `70B`). Placeholder labels with spaces/ellipses pass through.
fn prettify_model_id(model: &str) -> String {
    if model.contains(' ') || model.contains('…') || model.contains('/') {
        return model.to_string();
    }

    fn is_acronym(part: &str) -> bool {
        // Well-known initialisms that contain vowels and would otherwise be
        // title-cased as words.
        const KNOWN: &[&str] = &["oss", "ai", "moe", "vl", "it", "fp8", "awq", "exp"];
        if KNOWN.contains(&part.to_ascii_lowercase().as_str()) {
            return true;
        }
        // Short, all-alphabetic, and vowel-less segments read as initialisms:
        // glm, gpt, qwq, llm. Anything with a vowel (pro, max, mini, fable)
        // reads as a word and gets normal title-casing.
        part.len() <= 4
            && part.chars().all(|c| c.is_ascii_alphabetic())
            && !part
                .chars()
                .any(|c| matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y'))
    }

    fn is_param_size(part: &str) -> bool {
        // 70b / 8x7b / 32k style size or context markers.
        part.len() >= 2
            && part
                .chars()
                .last()
                .is_some_and(|c| matches!(c.to_ascii_lowercase(), 'b' | 'm' | 'k'))
            && part[..part.len() - 1]
                .chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == 'x')
            && part.chars().any(|c| c.is_ascii_digit())
    }

    let parts: Vec<String> = model
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        // Drop date/snapshot suffixes like 20241022.
        .filter(|part| !(part.len() >= 6 && part.chars().all(|c| c.is_ascii_digit())))
        .map(|part| {
            if is_acronym(part) || is_param_size(part) {
                return part.to_uppercase();
            }
            let mut chars = part.chars();
            match chars.next() {
                Some(first) if first.is_ascii_alphabetic() => {
                    first.to_uppercase().chain(chars).collect::<String>()
                }
                Some(first) => first.to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect();
    if parts.is_empty() {
        model.to_string()
    } else {
        parts.join(" ")
    }
}

/// Final display name for the header model line: curated pretty names first
/// (Claude 4.5 Opus, GPT-5.2 Codex), generic title-cased prettification otherwise.
fn header_model_display_name(model: &str, provider_name: &str) -> String {
    let raw = model.trim();

    // Claude family ids ("claude-opus-4-6", "claude-3-5-sonnet-latest",
    // "claude-haiku-4.5") render as "Claude <version> <Family>" for any
    // version, instead of only the hardcoded 3.5/4.5 cases.
    if raw.starts_with("claude") {
        for family in ["opus", "sonnet", "haiku"] {
            if raw.contains(family) {
                let family_pretty = capitalize(family);
                let version = claude_version_segment(raw, family);
                return match version {
                    Some(version) => format!("Claude {} {}", version, family_pretty),
                    None => format!("Claude {}", family_pretty),
                };
            }
        }
    }

    // GPT ids are formatted from the raw segments ("gpt-5.1-codex-max" ->
    // "GPT-5.1 Codex Max") rather than the legacy mashed short form, which
    // produced "GPT-5.1codexmax"-style names.
    if let Some(rest) = raw.strip_prefix("gpt-")
        && rest.starts_with(|c: char| c.is_ascii_digit())
    {
        let mut segments = rest.split('-');
        let version = segments.next().unwrap_or("");
        let mut name = format!("GPT-{}", version);
        for segment in segments {
            if segment.is_empty() {
                continue;
            }
            let pretty = prettify_model_id(segment);
            name.push(' ');
            name.push_str(&pretty);
        }
        return name;
    }

    let short_model = shorten_model_name(raw);
    let curated = format_model_name(&short_model, provider_name);
    if curated == short_model {
        // No curated pretty name matched; title-case the raw model id
        // instead of showing the mangled short form (`claudefable5`).
        prettify_model_id(raw)
    } else {
        curated
    }
}

/// Extract the version from a Claude model id, e.g. "claude-opus-4-6" -> "4.6",
/// "claude-3-5-sonnet-latest" -> "3.5", "claude-haiku-4.5" -> "4.5". Snapshot
/// dates (6+ digit runs) are ignored.
fn claude_version_segment(raw: &str, family: &str) -> Option<String> {
    let digits: Vec<&str> = raw
        .split(['-', '_'])
        .filter(|part| *part != family)
        .filter(|part| {
            !part.is_empty()
                && part.len() < 6
                && part.chars().all(|c| c.is_ascii_digit() || c == '.')
                && part.chars().any(|c| c.is_ascii_digit())
        })
        .collect();
    match digits.as_slice() {
        [] => None,
        [single] => Some(single.to_string()),
        [major, minor, ..] => Some(format!(
            "{}.{}",
            major.trim_matches('.'),
            minor.trim_matches('.')
        )),
    }
}

fn auth_dot_color(state: AuthState) -> Color {
    match state {
        AuthState::Available => rgb(100, 200, 100),
        AuthState::Expired => rgb(255, 200, 100),
        AuthState::NotConfigured => rgb(80, 80, 80),
    }
}

#[cfg(test)]
fn auth_dot_char(state: AuthState) -> &'static str {
    match state {
        AuthState::Available => "●",
        AuthState::Expired => "◐",
        AuthState::NotConfigured => "○",
    }
}

/// Authoritative active credential per dual-auth provider, resolved by the app
/// from the live provider/remote server. `None` entries mean "unknown, fall
/// back to the cached `AuthStatus` + env heuristic".
#[derive(Clone, Copy, Default)]
pub(super) struct ActiveCredentialOverrides {
    anthropic: Option<crate::auth::ActiveCredential>,
    openai: Option<crate::auth::ActiveCredential>,
}

impl ActiveCredentialOverrides {
    fn from_app(app: &dyn TuiState) -> Self {
        Self {
            anthropic: app.active_dual_credential(jcode_provider_core::ActiveProvider::Claude),
            openai: app.active_dual_credential(jcode_provider_core::ActiveProvider::OpenAI),
        }
    }

    fn get(
        &self,
        provider: jcode_provider_core::ActiveProvider,
    ) -> Option<crate::auth::ActiveCredential> {
        match provider {
            jcode_provider_core::ActiveProvider::Claude => self.anthropic,
            jcode_provider_core::ActiveProvider::OpenAI => self.openai,
            _ => None,
        }
    }
}

fn header_provider_auth_tag(
    name: &str,
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
) -> &'static str {
    let runtime_provider = std::env::var("JCODE_RUNTIME_PROVIDER").ok();

    // Anthropic and OpenAI share one credential-resolution source of truth so
    // the header tag never drifts from the info widget / model-switch line. We
    // route through the canonical ActiveProvider rather than matching display
    // strings, which is how this surface previously broke (name == "claude"
    // never matched a "anthropic"-only arm and the tag silently vanished).
    if let Some(provider) = jcode_provider_core::parse_provider_hint(name) {
        use crate::auth::{ActiveCredential, resolve_dual_credential_auth};
        match resolve_dual_credential_auth(provider, auth, runtime_provider.as_deref()) {
            Some(resolved) => {
                // The app's live answer wins over the env heuristic; the env
                // var is frequently absent in the TUI client process.
                let credential = active.get(provider).unwrap_or(resolved.active);
                // Report exactly the credential the next request will use. The
                // "both configured" inventory now lives in the auth status line
                // (`oauth*+key`), so this tag never claims two credentials at
                // once -- that ambiguity is how "Claude OAuth" and "API key"
                // used to contradict each other across surfaces.
                return match credential {
                    ActiveCredential::OAuth => "oauth",
                    ActiveCredential::ApiKey => "api-key",
                };
            }
            // Provider recognized but no credentials configured: no tag.
            None if matches!(
                provider,
                jcode_provider_core::ActiveProvider::Claude
                    | jcode_provider_core::ActiveProvider::OpenAI
            ) =>
            {
                return "";
            }
            None => {}
        }
    }

    match name {
        "copilot" => {
            if auth.copilot_has_api_token {
                "oauth"
            } else {
                ""
            }
        }
        "openrouter" | "openai-compatible" => "api-key",
        other
            if crate::provider_catalog::resolve_openai_compatible_profile_selection(other)
                .is_some()
                || crate::provider_catalog::openai_compatible_profile_id_for_display_name(
                    other,
                )
                .is_some() =>
        {
            "api-key"
        }
        _ => "",
    }
}

fn header_provider_label(
    provider_name: &str,
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
) -> String {
    let trimmed = provider_name.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let name = trimmed.to_lowercase();
    let auth_tag = header_provider_auth_tag(&name, auth, active);
    if auth_tag.is_empty() {
        name
    } else {
        format!("{}:{}", auth_tag, name)
    }
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
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

#[cfg(test)]
pub(super) fn build_persistent_header(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let auth = app.auth_status();
    let active = ActiveCredentialOverrides::from_app(app);
    fit_header_lines(
        build_persistent_header_with_auth(app, width, &auth, active),
        width,
    )
}

fn build_persistent_header_with_auth(
    app: &dyn TuiState,
    width: u16,
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
) -> Vec<Line<'static>> {
    let model = app.provider_model();
    let session_name = app.session_display_name().unwrap_or_default();
    let server_name = app.server_display_name();
    // The client line is identified by its session name, so show that name's
    // icon (e.g. "ram" -> 🐏). Previously a remote http/ws connection icon
    // (🌐/🔌) replaced it entirely, which hid the name icon for every remote
    // client. Keep the connection icon as a separate trailing hint instead.
    let icon = crate::id::session_icon(&session_name);
    let connection_icon = connection_type_icon(app.connection_type().as_deref());
    let nice_model = header_model_display_name(&model, &app.provider_name());
    let align = Alignment::Left;
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
    if server_update {
        status_items.push("srv↑");
    }
    if client_update {
        status_items.push("cli↑");
    }
    if let Some(badge) = crate::perf::profile().tier.badge() {
        status_items.push(badge);
    }

    // Labeled versions for the `server:` / `client:` lines. Lots of users run
    // mismatched client/server binaries, so both lines carry their own version
    // label (and highlight on mismatch) instead of relying on the single
    // ambiguous version line at the bottom.
    let server_version_full = app.server_display_version();
    let client_version_full = server_name
        .as_ref()
        .map(|_| jcode_build_meta::version().to_string());
    let version_mismatch = matches!(
        (&server_version_full, &client_version_full),
        (Some(server), Some(client)) if server.trim() != client.trim()
    );
    let include_hash = version_mismatch
        && matches!(
            (&server_version_full, &client_version_full),
            (Some(server), Some(client))
                if compact_version_label(server) == compact_version_label(client)
        );
    let version_style = if version_mismatch {
        Style::default().fg(rgb(255, 200, 100))
    } else {
        Style::default().fg(dim_color())
    };
    let server_version_label = server_version_full
        .as_deref()
        .map(|version| header_version_label(version, include_hash));
    let client_version_label = client_version_full
        .as_deref()
        .map(|version| header_version_label(version, include_hash));

    // First line: `jcode` (+ `self-dev` when running a dev/canary build),
    // followed by any remaining status badges rendered dimly.
    {
        lines.push(product_header_line(is_canary, &status_items));
    }

    if let Some(server_name) = server_name.as_deref() {
        let server_icon = app.server_display_icon().unwrap_or_default();
        let server_text = if server_icon.is_empty() {
            format!("server: {}", capitalize(server_name))
        } else {
            format!("server: {} {}", capitalize(server_name), server_icon)
        };
        let mut spans = vec![Span::styled(
            server_text.clone(),
            Style::default().fg(dim_color()),
        )];
        if let Some(version) = server_version_label.as_deref() {
            let suffix = format!(" · {}", version);
            if server_text.chars().count() + suffix.chars().count() <= w {
                spans.push(Span::styled(suffix, version_style));
            }
        }
        lines.push(Line::from(spans).alignment(align));
    }

    if !session_name.is_empty() {
        let client_text = client_header_text(&session_name, icon, connection_icon);
        let mut spans = vec![Span::styled(
            client_text.clone(),
            Style::default().fg(dim_color()),
        )];
        if let Some(version) = client_version_label.as_deref() {
            let suffix = format!(" · {}", version);
            if client_text.chars().count() + suffix.chars().count() <= w {
                spans.push(Span::styled(suffix, version_style));
            }
        }
        lines.push(Line::from(spans).alignment(align));
    } else if is_remote && server_name.is_some() && app.remote_startup_phase_active() {
        // The session name arrives with the server's History event. Hold the
        // row now so the header does not grow by a line (and shove the whole
        // layout down) when it lands a few dozen milliseconds later.
        lines.push(
            Line::from(Span::styled(
                "client: connecting…".to_string(),
                Style::default().fg(dim_color()),
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

    // Single model line: dim active-route method on the left, styled model
    // name in the middle, dim upstream/hint detail after. This used to be a
    // second, unstyled line in the secondary header duplicating the model name.
    let model_is_placeholder = model_is_placeholder(&model);
    let provider_label = if model_is_placeholder {
        String::new()
    } else {
        header_provider_label(&app.provider_name(), auth, active)
    };
    let upstream = if model_is_placeholder {
        None
    } else {
        app.upstream_provider()
    };
    if !nice_model.is_empty() {
        lines.push(model_header_line(
            &nice_model,
            &provider_label,
            upstream.as_deref(),
            w,
            model_is_placeholder,
        ));
    }

    // When there is no server/client version labeling (standalone mode),
    // still surface the running version on the jcode line's own row.
    if client_version_label.is_none() {
        lines.push(standalone_version_line());
    }

    lines
}

#[cfg(test)]
pub(crate) fn build_header_lines(app: &dyn TuiState, width: u16) -> Vec<Line<'static>> {
    let auth = app.auth_status();
    let active = ActiveCredentialOverrides::from_app(app);
    fit_header_lines(
        build_header_lines_with_auth(app, width, &auth, active),
        width,
    )
}

fn build_header_lines_with_auth(
    app: &dyn TuiState,
    width: u16,
    auth: &AuthStatus,
    active: ActiveCredentialOverrides,
) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    let align = ratatui::layout::Alignment::Left;
    let w = width as usize;

    // Local startup inventory uses saved metadata only. SSH must never present
    // this machine's credentials as the remote server's inventory.
    let (login_heading, auth_lines) = if let Some(host) = crate::tui::ssh_remote_host() {
        // The native protocol reports the active route, not a complete remote
        // credential inventory. Do not render the laptop's (or an empty)
        // inventory as if it described providers configured on the server.
        // Only the daemon's credential-free OAuth account report is shown.
        match account_router::remote_lines(
            &app.provider_name(),
            app.startup_oauth_account_label().as_deref(),
        ) {
            Some(lines) if !lines.is_empty() => (
                format!("Account router on {host} · /account to manage · /login to add"),
                lines,
            ),
            _ => (format!("/login to authenticate on {host}"), Vec::new()),
        }
    } else {
        (
            "Account router · /account to manage · /login to add credential".to_string(),
            account_router::lines(
                auth,
                active,
                &app.provider_name(),
                app.startup_oauth_account_label().as_deref(),
            ),
        )
    };
    lines.push(
        Line::from(Span::styled(
            login_heading,
            Style::default().fg(dim_color()),
        ))
        .alignment(align),
    );
    for line in auth_lines {
        lines.push(line.alignment(align));
    }

    let mcps = app.mcp_servers();
    if !mcps.is_empty() {
        const MAX_MCPS: usize = 4;
        let shown: Vec<String> = mcps
            .iter()
            .take(MAX_MCPS)
            .map(|(name, count)| {
                if *count > 0 {
                    format!("{} ({} tools)", name, count)
                } else {
                    format!("{} (…)", name)
                }
            })
            .collect();
        let mut mcp_text = format!("mcp: {}", shown.join(", "));
        if mcps.len() > MAX_MCPS {
            mcp_text.push_str(&format!(" +{} more", mcps.len() - MAX_MCPS));
        }
        if mcp_text.chars().count() > w {
            mcp_text = format!("mcp: {} servers", mcps.len());
        }
        lines.push(
            Line::from(Span::styled(mcp_text, Style::default().fg(dim_color()))).alignment(align),
        );
    }

    let skills = app.available_skills();
    if !skills.is_empty() {
        const MAX_SKILLS: usize = 6;
        let shown: Vec<String> = skills
            .iter()
            .take(MAX_SKILLS)
            .map(|s| format!("/{}", s))
            .collect();
        let mut skills_text = format!("skills: {}", shown.join(" "));
        if skills.len() > MAX_SKILLS {
            skills_text.push_str(&format!(" +{} more", skills.len() - MAX_SKILLS));
        }
        if skills_text.chars().count() > w {
            skills_text = format!("skills: {} loaded", skills.len());
        }
        lines.push(
            Line::from(Span::styled(skills_text, Style::default().fg(dim_color())))
                .alignment(align),
        );
    }

    if let Some(dir) = app.working_dir() {
        let display_dir = abbreviate_home(&dir);
        let mut text = display_dir;
        if let Some(branch) = app.git_branch() {
            let with_branch = format!("{} ({})", text, branch);
            if with_branch.chars().count() <= w {
                text = with_branch;
            }
        }
        lines.push(
            Line::from(Span::styled(text, Style::default().fg(dim_color()))).alignment(align),
        );
    }

    lines.push(Line::from(""));
    lines
}

/// Build the "Updates" rounded box (unseen release notes) so it can be
/// rendered inside the top padding above the header. `max_lines` bounds the
/// total height including the box borders; entries beyond the budget are
/// collapsed into a "…N more" line. Returns an empty vec when there are no
/// unseen entries or the budget/width is too small for a box.
pub(super) fn build_updates_box_lines(width: u16, max_lines: usize) -> Vec<Line<'static>> {
    let w = width as usize;
    if w <= 20 || max_lines < 3 {
        return Vec::new();
    }
    let new_entries = unseen_changelog_entries();
    if new_entries.is_empty() {
        return Vec::new();
    }

    // Budget for content lines inside the box (borders take 2 lines).
    let content_budget = (max_lines - 2).min(8);
    let has_more = new_entries.len() > content_budget;
    let display_count = if has_more {
        content_budget.saturating_sub(1)
    } else {
        new_entries.len()
    };

    let mut content: Vec<Line> = Vec::new();
    for entry in new_entries.iter().take(display_count) {
        content.push(Line::from(Span::styled(
            format!("• {}", entry),
            Style::default().fg(dim_color()),
        )));
    }
    if has_more {
        content.push(Line::from(Span::styled(
            format!(
                "  …{} more · /changelog to see all",
                new_entries.len() - display_count
            ),
            Style::default().fg(dim_color()),
        )));
    }
    if content.is_empty() {
        return Vec::new();
    }

    render_rounded_box(
        "Updates",
        content,
        w.saturating_sub(2),
        Style::default().fg(dim_color()),
    )
    .into_iter()
    .map(|line| line.alignment(Alignment::Left))
    .collect()
}

/// Build both header sections from one authentication snapshot. Credential
/// discovery can touch several files on Windows, so the render path must not
/// repeat it for the persistent and secondary portions of the same frame.
pub(in crate::tui) fn build_header_sections(
    app: &dyn TuiState,
    width: u16,
) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let start = std::time::Instant::now();
    let auth = app.auth_status();
    let auth_ms = start.elapsed().as_millis();
    let active = ActiveCredentialOverrides::from_app(app);
    let active_ms = start.elapsed().as_millis() - auth_ms;
    let persistent = fit_header_lines(
        build_persistent_header_with_auth(app, width, &auth, active),
        width,
    );
    let persistent_ms = start.elapsed().as_millis() - auth_ms - active_ms;
    let secondary = fit_header_lines(
        build_header_lines_with_auth(app, width, &auth, active),
        width,
    );
    let total_ms = start.elapsed().as_millis();
    if total_ms >= 20 {
        crate::logging::info(&format!(
            "[TIMING] header_sections: total={total_ms}ms auth={auth_ms}ms active={active_ms}ms persistent={persistent_ms}ms secondary={}ms",
            total_ms - auth_ms - active_ms - persistent_ms
        ));
    }
    (persistent, secondary)
}

#[cfg(test)]
#[path = "ui_header/header_tests.rs"]
mod header_tests;
