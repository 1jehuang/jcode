use super::*;
use crate::auth::{AuthState, AuthStatus, ProviderAuth};
use crate::message::Message;
use crate::provider::{EventStream, Provider};
use crate::tool::Registry;
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::OnceLock;

#[test]
fn header_rows_clip_unicode_and_controls_without_losing_style() {
    let style = Style::default().fg(Color::Magenta);
    let source = vec![
        Line::from(vec![
            Span::styled("账号 👩‍💻 e\u{301} ", style),
            Span::raw("/very/long/path\nwith\ttabs and a long account email@example.com"),
        ])
        .alignment(Alignment::Left),
    ];
    for width in [0u16, 1, 2, 3, 8, 18, 40, 80] {
        let fitted = fit_header_lines(source.clone(), width);
        assert_eq!(fitted.len(), 1);
        assert!(fitted[0].width() <= usize::from(width.saturating_sub(1)));
        assert_eq!(fitted[0].alignment, Some(Alignment::Left));
        assert!(
            fitted[0]
                .spans
                .iter()
                .all(|span| !span.content.chars().any(char::is_control))
        );
        if width >= 8 {
            assert_eq!(fitted[0].spans[0].style, style);
        }
    }
}

#[test]
fn startup_header_never_adds_wraparound_rows_at_narrow_widths() {
    let app = create_test_app();
    for width in [2u16, 8, 18, 32, 40, 60, 80, 120] {
        let (mut lines, secondary) = build_header_sections(&app, width);
        lines.extend(secondary);
        for line in lines {
            let budget = usize::from(width.saturating_sub(1));
            assert!(line.width() <= budget, "width={width}: {line:?}");
            let wrapped = jcode_tui_markdown::wrap_line(line, budget);
            assert_eq!(
                wrapped.len(),
                1,
                "header must stay one visual row at width={width}"
            );
        }
    }
}

#[test]
fn changelog_override_is_cleared_after_a_render_test_panics() {
    let _lock = crate::tui::ui::render_state_test_lock();
    let panic = std::panic::catch_unwind(|| {
        let _fixture = scoped_unseen_changelog_entries_override_for_tests(vec![
            "temporary changelog entry".to_owned(),
        ]);
        assert_eq!(unseen_changelog_entries(), ["temporary changelog entry"]);
        panic!("injected render failure");
    });
    assert!(panic.is_err());
    assert!(
        unseen_changelog_entries_override()
            .lock()
            .unwrap()
            .is_none(),
        "a failed render test must not leak its changelog fixture"
    );
}

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
        Err(anyhow::anyhow!(
            "Mock provider should not be used for streaming completions in ui header tests"
        ))
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
    crate::tui::app::App::new_for_test_harness(provider, registry)
}

#[test]
fn left_aligned_mode_keeps_persistent_header_left_aligned() {
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
            .all(|line| line.alignment == Some(Alignment::Left)),
        "persistent header should be left aligned: {non_empty:?}"
    );
}

#[test]
fn left_aligned_mode_keeps_secondary_header_left_aligned() {
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
            .all(|line| line.alignment == Some(Alignment::Left)),
        "header detail lines should be left aligned: {non_empty:?}"
    );
}

#[test]
fn combined_header_sections_match_individual_builders() {
    let app = create_test_app();
    let (persistent, secondary) = build_header_sections(&app, 80);

    assert_eq!(persistent, build_persistent_header(&app, 80));
    assert_eq!(secondary, build_header_lines(&app, 80));
}

#[test]
fn version_display_candidates_compact_for_narrow_width() {
    let rendered = choose_header_candidate(8, version_display_candidates());
    // Version-agnostic: at width 8 only the bare minor semver fits.
    assert_eq!(rendered, semver_minor());
}

fn rendered_header_lines(app: &crate::tui::app::App, width: u16) -> Vec<String> {
    build_persistent_header(app, width)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn persistent_header_labels_server_and_client_versions() {
    let mut app = create_test_app();
    app.set_remote_server_identity_for_tests(
        Some("blazing"),
        Some("🔥"),
        Some("v0.14.2-dev (old1234)"),
        Some("session_fox_1705012345678"),
    );

    let lines = rendered_header_lines(&app, 120);
    let server_line = lines
        .iter()
        .find(|line| line.contains("server:"))
        .expect("server line");
    let client_line = lines
        .iter()
        .find(|line| line.contains("client:"))
        .expect("client line");

    assert!(
        server_line.contains("server: Blazing 🔥 · v0.14.2-dev"),
        "server line should carry the server version: {server_line}"
    );
    let client_version = compact_version_label(jcode_build_meta::version());
    assert!(
        client_line.contains("client: Fox"),
        "client line should keep the session name: {client_line}"
    );
    assert!(
        client_line.contains(&format!("· {}", client_version)),
        "client line should carry the client version: {client_line}"
    );
}

#[test]
fn persistent_header_keeps_git_hash_when_semvers_match_but_builds_differ() {
    let mut app = create_test_app();
    let client_semver = compact_version_label(jcode_build_meta::version());
    let fake_server_version = format!("{} (0000000)", client_semver);
    app.set_remote_server_identity_for_tests(
        Some("blazing"),
        None,
        Some(&fake_server_version),
        Some("session_fox_1705012345678"),
    );

    let lines = rendered_header_lines(&app, 160);
    let server_line = lines
        .iter()
        .find(|line| line.contains("server:"))
        .expect("server line");
    let client_line = lines
        .iter()
        .find(|line| line.contains("client:"))
        .expect("client line");

    assert!(
        server_line.contains("(0000000)"),
        "same-semver mismatch should keep the server git hash: {server_line}"
    );
    assert!(
        client_line.contains(&format!("· {}", jcode_build_meta::version())),
        "same-semver mismatch should keep the client git hash: {client_line}"
    );
}

#[test]
fn persistent_header_omits_version_suffix_when_too_narrow() {
    let mut app = create_test_app();
    app.set_remote_server_identity_for_tests(
        Some("blazing"),
        Some("🔥"),
        Some("v0.14.2-dev (old1234)"),
        Some("session_fox_1705012345678"),
    );

    let lines = rendered_header_lines(&app, 18);
    let server_line = lines
        .iter()
        .find(|line| line.contains("server:"))
        .expect("server line");
    assert!(
        !server_line.contains("v0.14.2"),
        "narrow widths should drop the version suffix: {server_line}"
    );
}

#[test]
fn persistent_header_local_mode_has_no_version_labels() {
    let app = create_test_app();
    let lines = rendered_header_lines(&app, 120);
    assert!(
        !lines.iter().any(|line| line.contains("server:")),
        "local mode should not render a server line: {lines:?}"
    );
    assert!(
        !lines
            .iter()
            .any(|line| line.contains("client:") && line.contains(" · v")),
        "local mode client line should not carry a version label: {lines:?}"
    );
}

#[test]
fn persistent_header_client_line_shows_name_icon_with_connection_hint() {
    let mut app = create_test_app();
    app.set_remote_server_identity_for_tests(
        Some("blazing"),
        Some("🔥"),
        Some("v0.14.2-dev (old1234)"),
        Some("session_ram_1705012345678"),
    );
    app.set_connection_type_for_tests(Some("https/sse"));

    let lines = rendered_header_lines(&app, 120);
    let client_line = lines
        .iter()
        .find(|line| line.contains("client:"))
        .expect("client line");

    // The session name's own icon (ram -> 🐏) must be present rather than
    // being replaced by the connection icon.
    assert!(
        client_line.contains("client: Ram 🐏"),
        "client line should show the name icon: {client_line}"
    );
    // The connection icon is kept as a trailing hint, not a replacement.
    assert!(
        client_line.contains('🌐'),
        "client line should keep the connection hint icon: {client_line}"
    );
}

#[test]
fn persistent_header_client_line_has_no_connection_hint_when_unknown() {
    let mut app = create_test_app();
    app.set_remote_server_identity_for_tests(
        Some("blazing"),
        Some("🔥"),
        Some("v0.14.2-dev (old1234)"),
        Some("session_fox_1705012345678"),
    );
    app.set_connection_type_for_tests(None);

    let lines = rendered_header_lines(&app, 120);
    let client_line = lines
        .iter()
        .find(|line| line.contains("client:"))
        .expect("client line");

    assert!(
        client_line.contains("client: Fox 🦊"),
        "client line should show the name icon: {client_line}"
    );
    assert!(
        !client_line.contains('🌐') && !client_line.contains('🔌'),
        "client line should not carry a connection hint when unknown: {client_line}"
    );
}

#[test]
fn prettify_model_id_title_cases_unknown_models() {
    assert_eq!(prettify_model_id("claude-fable-5"), "Claude Fable 5");
    assert_eq!(prettify_model_id("grok-code-fast-1"), "Grok Code Fast 1");
    assert_eq!(prettify_model_id("kimi_k2"), "Kimi K2");
    assert_eq!(
        prettify_model_id("gemini-3-pro-preview"),
        "Gemini 3 Pro Preview"
    );
    assert_eq!(prettify_model_id("deepseek-chat"), "Deepseek Chat");
    assert_eq!(
        prettify_model_id("mistral-large-2411"),
        "Mistral Large 2411"
    );
    assert_eq!(prettify_model_id("o3-mini"), "O3 Mini");
    // Vowel-less short segments read as acronyms.
    assert_eq!(prettify_model_id("glm-4.6"), "GLM 4.6");
    assert_eq!(prettify_model_id("qwq-32b"), "QWQ 32B");
    // Parameter sizes are uppercased.
    assert_eq!(prettify_model_id("llama-3.3-70b"), "Llama 3.3 70B");
    assert_eq!(prettify_model_id("mixtral-8x7b"), "Mixtral 8X7B");
    // Long digit runs (snapshot dates) are dropped.
    assert_eq!(
        prettify_model_id("claude-fable-5-20260101"),
        "Claude Fable 5"
    );
    // Placeholders and slashed ids pass through untouched.
    assert_eq!(prettify_model_id("loading session…"), "loading session…");
    assert_eq!(
        prettify_model_id("deepseek/deepseek-chat"),
        "deepseek/deepseek-chat"
    );
    // Degenerate inputs survive.
    assert_eq!(prettify_model_id(""), "");
    assert_eq!(prettify_model_id("-"), "-");
}

#[test]
fn header_model_display_name_sweeps_real_model_catalog() {
    // End-to-end through shorten_model_name + format_model_name +
    // prettify_model_id, over the model ids jcode actually routes.
    let cases = [
        // Anthropic
        ("claude-opus-4-5-20251101", "Claude 4.5 Opus"),
        ("claude-opus-4.6", "Claude 4.6 Opus"),
        ("claude-opus-4-8", "Claude 4.8 Opus"),
        ("claude-sonnet-4-5", "Claude 4.5 Sonnet"),
        ("claude-sonnet-4", "Claude 4 Sonnet"),
        ("claude-3-5-sonnet-latest", "Claude 3.5 Sonnet"),
        ("claude-haiku-4-5", "Claude 4.5 Haiku"),
        ("claude-fable-5", "Claude Fable 5"),
        // OpenAI
        ("gpt-5.2-codex", "GPT-5.2 Codex"),
        ("gpt-5.1-codex-max", "GPT-5.1 Codex Max"),
        ("gpt-5.3-codex-spark", "GPT-5.3 Codex Spark"),
        ("gpt-5-mini", "GPT-5 Mini"),
        ("gpt-5.1-chat-latest", "GPT-5.1 Chat Latest"),
        ("gpt-4o", "GPT-4o"),
        ("gpt-4o-mini", "GPT-4o Mini"),
        ("gpt-oss-120b", "GPT OSS 120B"),
        ("o3-mini", "O3 Mini"),
        ("o4-mini", "O4 Mini"),
        // Google
        ("gemini-3-pro-preview", "Gemini 3 Pro Preview"),
        ("gemini-2.5-flash", "Gemini 2.5 Flash"),
        // xAI / Moonshot / Zhipu / DeepSeek / Minimax
        ("grok-code-fast-1", "Grok Code Fast 1"),
        ("kimi-k2.5", "Kimi K2.5"),
        ("kimi-k2p5-turbo", "Kimi K2p5 Turbo"),
        ("glm-4.6", "GLM 4.6"),
        ("deepseek-v4-flash", "Deepseek V4 Flash"),
        ("minimax-m2.7", "Minimax M2.7"),
        // Meta / Mistral / Qwen / community
        ("llama-3.3-70b", "Llama 3.3 70B"),
        ("mixtral-8x7b", "Mixtral 8X7B"),
        ("devstral-medium-2507", "Devstral Medium 2507"),
        ("qwen3-coder-plus", "Qwen3 Coder Plus"),
        ("composer-1.5", "Composer 1.5"),
        ("llama-3.1-8b-instant", "Llama 3.1 8B Instant"),
    ];
    for (input, expected) in cases {
        assert_eq!(
            header_model_display_name(input, ""),
            expected,
            "model id {input:?}"
        );
    }

    // Slashed ids keep the provider label form.
    assert_eq!(
        header_model_display_name("deepseek/deepseek-chat", "OpenRouter"),
        "OpenRouter: deepseek/deepseek-chat"
    );
    // Placeholders pass through untouched.
    assert_eq!(
        header_model_display_name("loading session…", ""),
        "loading session…"
    );
    assert_eq!(header_model_display_name("connected", ""), "Connected");
}

#[test]
fn compact_version_label_strips_hash_suffix() {
    assert_eq!(
        compact_version_label("v0.25.19-dev (7e261bcc, dirty)"),
        "v0.25.19-dev"
    );
    assert_eq!(compact_version_label("v0.25.19 (abc1234)"), "v0.25.19");
    assert_eq!(compact_version_label(" v0.25.19 "), "v0.25.19");
}

#[test]
fn configured_auth_count_includes_non_model_auth_surfaces() {
    let auth = AuthStatus {
        jcode: AuthState::Available,
        anthropic: ProviderAuth {
            state: AuthState::Expired,
            has_oauth: true,
            oauth_state: AuthState::Expired,
            has_api_key: false,
        },
        azure: AuthState::Available,
        google: AuthState::Available,
        ..AuthStatus::default()
    };

    assert_eq!(configured_auth_count(&auth), 4);
}

#[test]
fn header_provider_auth_tag_reports_active_credential_for_openai() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_RUNTIME_PROVIDER");
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    let auth = AuthStatus {
        openai: AuthState::Available,
        openai_has_oauth: true,
        openai_has_api_key: true,
        ..AuthStatus::default()
    };

    // Auto mode prefers OAuth; the tag must report only the credential in
    // use (the auth inventory line carries the "both configured" detail).
    assert_eq!(
        header_provider_auth_tag("openai", &auth, ActiveCredentialOverrides::default()),
        "oauth"
    );
    if let Some(value) = prev {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", value);
    }
}

#[test]
fn header_provider_auth_tag_prefers_app_resolved_credential_over_env() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_RUNTIME_PROVIDER");
    // The TUI client usually does not inherit JCODE_RUNTIME_PROVIDER, so the
    // env heuristic would answer "oauth" here; the app's resolution must win.
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    let both = AuthStatus {
        anthropic: ProviderAuth {
            // `state` must be set alongside the credential booleans:
            // `build_auth_status_lines` filters `NotConfigured` providers out
            // and falls back to the full "no credentials" list (issue #654).
            state: AuthState::Available,
            has_oauth: true,
            oauth_state: AuthState::Available,
            has_api_key: true,
        },
        ..AuthStatus::default()
    };
    let overrides = ActiveCredentialOverrides {
        anthropic: Some(crate::auth::ActiveCredential::ApiKey),
        openai: None,
    };
    assert_eq!(
        header_provider_auth_tag("anthropic", &both, overrides),
        "api-key"
    );
    if let Some(value) = prev {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", value);
    }
}

#[test]
fn header_provider_auth_tag_honors_runtime_selection_and_oauth_first() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_RUNTIME_PROVIDER");

    let both = AuthStatus {
        anthropic: ProviderAuth {
            has_oauth: true,
            has_api_key: true,
            ..Default::default()
        },
        ..AuthStatus::default()
    };

    // Explicit API-key selection wins even when OAuth is available.
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-api");
    assert_eq!(
        header_provider_auth_tag("anthropic", &both, ActiveCredentialOverrides::default()),
        "api-key"
    );

    // Explicit OAuth selection.
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude");
    assert_eq!(
        header_provider_auth_tag("anthropic", &both, ActiveCredentialOverrides::default()),
        "oauth"
    );

    // Auto (unset) prefers OAuth when both credentials are present.
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    assert_eq!(
        header_provider_auth_tag("anthropic", &both, ActiveCredentialOverrides::default()),
        "oauth"
    );

    // The "claude" display name resolves to the same Anthropic tagging.
    assert_eq!(
        header_provider_auth_tag("claude", &both, ActiveCredentialOverrides::default()),
        "oauth"
    );
    crate::env::set_var("JCODE_RUNTIME_PROVIDER", "claude-api");
    assert_eq!(
        header_provider_auth_tag("claude", &both, ActiveCredentialOverrides::default()),
        "api-key"
    );
    crate::env::remove_var("JCODE_RUNTIME_PROVIDER");

    // Auto falls back to the API key when no OAuth credential exists.
    let api_only = AuthStatus {
        anthropic: ProviderAuth {
            has_oauth: false,
            has_api_key: true,
            ..Default::default()
        },
        ..AuthStatus::default()
    };
    assert_eq!(
        header_provider_auth_tag("anthropic", &api_only, ActiveCredentialOverrides::default()),
        "api-key"
    );

    if let Some(value) = prev {
        crate::env::set_var("JCODE_RUNTIME_PROVIDER", value);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_PROVIDER");
    }
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
    // Reads model/provider env-derived state: without the env lock, the
    // sibling test that sets JCODE_MODEL=gpt-5.4 mid-flight leaks into this
    // render and the "loading session…" placeholder never appears. The
    // startup-phase label is also only rendered when no model hint is
    // known, so neutralize JCODE_MODEL/JCODE_PROVIDER for the duration
    // ("unknown" also suppresses the shared test home's config
    // default_model fallback, which another test may have persisted).
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "unknown");
    crate::env::remove_var("JCODE_PROVIDER");

    let mut app = crate::tui::app::App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

    // The model line lives in the persistent header now; the startup phase
    // label renders there without a bogus "(unknown)" provider tag.
    let lines = build_persistent_header(&app, 80);
    let rendered = lines
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();

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

    assert!(rendered.contains("loading session…"), "{rendered}");
    assert!(!rendered.contains("(unknown)"));
    assert!(!rendered.contains("(remote)"));
}

#[test]
fn build_header_lines_hides_secondary_placeholder_during_brief_connecting_phase() {
    // Same env sensitivity as the placeholder test above: JCODE_MODEL /
    // JCODE_PROVIDER mutations from sibling tests change what renders.
    let _guard = crate::storage::lock_test_env();
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
fn format_model_name_labels_slashed_models_with_active_provider() {
    // Regression for issue #329: a NVIDIA NIM model must be labeled with the
    // active provider's display name, not the fixed "OpenRouter" aggregator.
    assert_eq!(
        format_model_name("nvidia/nemotron-3-super-120b-a12b", "NVIDIA NIM"),
        "NVIDIA NIM: nvidia/nemotron-3-super-120b-a12b"
    );
    // The public aggregator still reads "OpenRouter".
    assert_eq!(
        format_model_name("anthropic/claude-sonnet-4", "OpenRouter"),
        "OpenRouter: anthropic/claude-sonnet-4"
    );
    // Missing provider name falls back to "OpenRouter" rather than an empty label.
    assert_eq!(
        format_model_name("deepseek/deepseek-chat", ""),
        "OpenRouter: deepseek/deepseek-chat"
    );
    // Non-slashed models are unaffected by the provider label.
    assert_eq!(
        format_model_name("claude-opus-4-6", "OpenRouter"),
        "Claude Opus"
    );
}
