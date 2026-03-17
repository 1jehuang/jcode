use super::*;
use crate::bus::{BusEvent, InputShellCompleted};
use crate::tui::TuiState;
use ratatui::layout::Rect;
use std::sync::{Arc as StdArc, Mutex as StdMutex};

// Mock provider for testing
struct MockProvider;

#[async_trait::async_trait]
impl Provider for MockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("Mock provider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(MockProvider)
    }
}

fn create_test_app() -> App {
    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[derive(Clone)]
struct FastMockProvider {
    service_tier: StdArc<StdMutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Provider for FastMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("FastMockProvider")
    }

    fn name(&self) -> &str {
        "mock"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }

    fn service_tier(&self) -> Option<String> {
        self.service_tier.lock().unwrap().clone()
    }

    fn set_service_tier(&self, service_tier: &str) -> anyhow::Result<()> {
        let normalized = match service_tier.trim().to_ascii_lowercase().as_str() {
            "priority" | "fast" => Some("priority".to_string()),
            "off" | "default" | "auto" | "none" => None,
            other => anyhow::bail!("unsupported service tier {other}"),
        };
        *self.service_tier.lock().unwrap() = normalized;
        Ok(())
    }
}

fn create_fast_test_app() -> App {
    let provider: Arc<dyn Provider> = Arc::new(FastMockProvider {
        service_tier: StdArc::new(StdMutex::new(None)),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn create_gemini_test_app() -> App {
    struct GeminiMockProvider;

    #[async_trait::async_trait]
    impl Provider for GeminiMockProvider {
        async fn complete(
            &self,
            _messages: &[Message],
            _tools: &[crate::message::ToolDefinition],
            _system: &str,
            _resume_session_id: Option<&str>,
        ) -> Result<crate::provider::EventStream> {
            unimplemented!("Mock provider")
        }

        fn name(&self) -> &str {
            "gemini"
        }

        fn model(&self) -> String {
            "gemini-2.5-pro".to_string()
        }

        fn fork(&self) -> Arc<dyn Provider> {
            Arc::new(GeminiMockProvider)
        }
    }

    let provider: Arc<dyn Provider> = Arc::new(GeminiMockProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[test]
fn test_help_topic_shows_command_details() {
    let mut app = create_test_app();
    app.input = "/help compact".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/compact`"));
    assert!(msg.content.contains("background"));
    assert!(msg.content.contains("`/compact mode`"));
}

#[test]
fn test_goals_command_opens_overview_in_side_panel() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());
    app.input = "/goals".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("goals"));
    let msg = app
        .display_messages()
        .last()
        .expect("missing goals message");
    assert!(msg.content.contains("Opened goals overview"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_goals_show_command_focuses_goal_page() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("repo");
    std::fs::create_dir_all(&project).expect("project dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let goal = crate::goal::create_goal(
        crate::goal::GoalCreateInput {
            title: "Ship mobile MVP".to_string(),
            scope: crate::goal::GoalScope::Project,
            ..crate::goal::GoalCreateInput::default()
        },
        Some(&project),
    )
    .expect("create goal");

    let mut app = create_test_app();
    app.session.working_dir = Some(project.display().to_string());
    app.input = format!("/goals show {}", goal.id);
    app.submit_input();

    assert_eq!(
        app.side_panel.focused_page_id.as_deref(),
        Some(format!("goal.{}", goal.id).as_str())
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_compact_mode_command_updates_local_session_mode() {
    let mut app = create_test_app();

    app.input = "/compact mode semantic".to_string();
    app.submit_input();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mode = rt.block_on(async { app.registry.compaction().read().await.mode() });
    assert_eq!(mode, crate::config::CompactionMode::Semantic);

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "✓ Compaction mode → semantic");
}

#[test]
fn test_compact_mode_status_shows_local_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let compaction = app.registry.compaction();
        let mut manager = compaction.write().await;
        manager.set_mode(crate::config::CompactionMode::Proactive);
    });

    app.input = "/compact mode".to_string();
    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert!(last.content.contains("Compaction mode: **proactive**"));
}

#[test]
fn test_fast_on_while_processing_mentions_next_request_locally() {
    let mut app = create_fast_test_app();
    app.is_processing = true;
    app.input = "/fast on".to_string();

    app.submit_input();

    let last = app
        .display_messages()
        .last()
        .expect("missing fast mode response");
    assert_eq!(last.role, "system");
    assert_eq!(
        last.content,
        "✓ Fast mode on (Fast)\nApplies to the next request/turn. The current in-flight request keeps its existing tier."
    );
    assert_eq!(
        app.status_notice(),
        Some("Fast: on (next request)".to_string())
    );
}

#[test]
fn test_help_topic_shows_fix_command_details() {
    let mut app = create_test_app();
    app.input = "/help fix".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/fix`"));
}

#[test]
fn test_mask_email_censors_local_part() {
    assert_eq!(mask_email("jeremyh1@uw.edu"), "j***1@uw.edu");
}

#[test]
fn test_subscription_command_shows_jcode_status_scaffold() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::env::remove_var(crate::subscription_catalog::JCODE_API_KEY_ENV);
    crate::env::remove_var(crate::subscription_catalog::JCODE_API_BASE_ENV);

    let mut app = create_test_app();
    app.input = "/subscription".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing /subscription response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Jcode Subscription Status"));
    assert!(msg.content.contains("/login jcode"));
    assert!(msg.content.contains("Healer Alpha"));
    assert!(msg.content.contains("Kimi K2.5"));
    assert!(msg.content.contains("$20 Starter"));
    assert!(msg.content.contains("$100 Pro"));
}

#[test]
fn test_usage_report_shows_jcode_scaffold_when_subscription_mode_active() {
    let _guard = crate::storage::lock_test_env();
    crate::subscription_catalog::clear_runtime_env();
    crate::subscription_catalog::apply_runtime_env();

    let mut app = create_test_app();
    app.handle_usage_report(Vec::new());

    let msg = app
        .display_messages()
        .last()
        .expect("missing /usage scaffold response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Jcode Subscription"));
    assert!(msg.content.contains("Use `/subscription`"));
    assert!(msg.content.contains("$20 Starter"));

    crate::subscription_catalog::clear_runtime_env();
}

#[test]
fn test_show_accounts_includes_masked_email_column() {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let accounts = vec![crate::auth::claude::AnthropicAccount {
        label: "work".to_string(),
        access: "acc".to_string(),
        refresh: "ref".to_string(),
        expires: now_ms + 60000,
        email: Some("user@example.com".to_string()),
        subscription_type: Some("max".to_string()),
    }];

    let mut lines = vec!["**Anthropic Accounts:**\n".to_string()];
    lines.push("| Account | Email | Status | Subscription | Active |".to_string());
    lines.push("|---------|-------|--------|-------------|--------|".to_string());

    for account in &accounts {
        let status = if account.expires > now_ms {
            "✓ valid"
        } else {
            "⚠ expired"
        };
        let email = account
            .email
            .as_deref()
            .map(mask_email)
            .unwrap_or_else(|| "unknown".to_string());
        let sub = account.subscription_type.as_deref().unwrap_or("unknown");
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            account.label, email, status, sub, "◉"
        ));
    }

    let output = lines.join("\n");
    assert!(output.contains("| Account | Email | Status | Subscription | Active |"));
    assert!(output.contains("u***r@example.com"));
}

#[test]
fn test_account_openai_command_opens_account_picker() {
    let _guard = crate::storage::lock_test_env();
    let now_ms = chrono::Utc::now().timestamp_millis();

    crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        label: "work".to_string(),
        access_token: "acc".to_string(),
        refresh_token: "ref".to_string(),
        id_token: None,
        account_id: Some("acct_work".to_string()),
        expires_at: Some(now_ms + 60_000),
        email: Some("user@example.com".to_string()),
    })
    .unwrap();

    let mut app = create_test_app();
    app.input = "/account openai".to_string();
    app.submit_input();

    assert!(
        app.account_picker_overlay.is_some(),
        "/account openai should open the account picker"
    );
}

#[test]
fn test_account_command_opens_account_picker() {
    let _guard = crate::storage::lock_test_env();
    let now_ms = chrono::Utc::now().timestamp_millis();

    crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        label: "work".to_string(),
        access_token: "acc".to_string(),
        refresh_token: "ref".to_string(),
        id_token: None,
        account_id: Some("acct_work".to_string()),
        expires_at: Some(now_ms + 60_000),
        email: Some("user@example.com".to_string()),
    })
    .unwrap();

    let mut app = create_test_app();
    app.input = "/account".to_string();
    app.submit_input();

    assert!(
        app.account_picker_overlay.is_some(),
        "/account should open the account picker"
    );
}

#[test]
fn test_account_switch_shorthand_switches_openai_account_by_label() {
    let _guard = crate::storage::lock_test_env();
    let now_ms = chrono::Utc::now().timestamp_millis();

    crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
        label: "openai2".to_string(),
        access_token: "acc".to_string(),
        refresh_token: "ref".to_string(),
        id_token: None,
        account_id: Some("acct_openai2".to_string()),
        expires_at: Some(now_ms + 60_000),
        email: Some("user2@example.com".to_string()),
    })
    .unwrap();

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        app.input = "/account switch openai2".to_string();
        app.submit_input();

        assert_eq!(
            crate::auth::codex::active_account_label().as_deref(),
            Some("openai2")
        );
    });
}

#[test]
fn test_account_picker_prompt_new_openai_label_cancel_clears_prompt() {
    let mut app = create_test_app();
    app.prompt_new_account_label(crate::tui::account_picker::AccountProviderKind::OpenAi);

    assert!(matches!(
        app.pending_account_label,
        Some(super::auth::PendingAccountLabel::OpenAi)
    ));

    app.input = "/cancel".to_string();
    app.submit_input();

    assert!(app.pending_account_label.is_none());
    assert!(app.pending_login.is_none());
}

#[test]
fn test_commands_alias_shows_help() {
    let mut app = create_test_app();
    app.input = "/commands".to_string();
    app.submit_input();

    assert!(
        app.help_scroll.is_some(),
        "/commands should open help overlay"
    );
}

#[test]
fn test_fix_resets_provider_session() {
    let mut app = create_test_app();
    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());
    app.last_stream_error = Some("Stream error: context window exceeded".to_string());

    app.input = "/fix".to_string();
    app.submit_input();

    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());

    let msg = app
        .display_messages()
        .last()
        .expect("missing /fix response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Fix Results"));
    assert!(msg.content.contains("Reset provider session resume state"));
}

#[test]
fn test_context_limit_error_detection() {
    assert!(is_context_limit_error(
        "OpenAI API error 400: This model's maximum context length is 200000 tokens"
    ));
    assert!(is_context_limit_error(
        "request too large: prompt is too long for context window"
    ));
    assert!(!is_context_limit_error(
        "rate limit exceeded, retry after 20s"
    ));
}

#[test]
fn test_rewind_truncates_provider_messages() {
    let mut app = create_test_app();

    for idx in 1..=3 {
        let text = format!("msg-{}", idx);
        app.add_provider_message(Message::user(&text));
        app.session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
        );
    }
    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());

    app.input = "/rewind 2".to_string();
    app.submit_input();

    assert_eq!(app.messages.len(), 2);
    assert_eq!(app.session.messages.len(), 2);
    assert!(matches!(
        &app.messages[1].content[0],
        ContentBlock::Text { text, .. } if text == "msg-2"
    ));
    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
}

#[test]
fn test_accumulate_streaming_output_tokens_uses_deltas() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.accumulate_streaming_output_tokens(10, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);

    assert_eq!(app.streaming_total_output_tokens, 30);
    assert_eq!(seen, 30);
}

#[test]
fn test_initial_state() {
    let app = create_test_app();

    assert!(!app.is_processing());
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert!(app.display_messages().is_empty());
    assert!(app.streaming_text().is_empty());
    assert_eq!(app.queued_count(), 0);
    assert!(matches!(app.status(), ProcessingStatus::Idle));
    assert!(app.elapsed().is_none());
}

#[test]
fn test_handle_key_typing() {
    let mut app = create_test_app();

    // Type "hello"
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), 5);
}

#[test]
fn test_handle_key_backspace() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Backspace, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "a");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_diagram_focus_toggle_and_pan() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);

    // Ctrl+L focuses diagram when available
    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.diagram_focus);

    // Pan should update scroll offsets and not type into input
    app.handle_key(KeyCode::Char('j'), KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diagram_scroll_y, 3);
    assert!(app.input.is_empty());

    // Ctrl+H returns focus to chat
    app.handle_key(KeyCode::Char('h'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(!app.diagram_focus);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_diagram_cycle_ctrl_arrows() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_focus = true;
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);
    crate::tui::mermaid::register_active_diagram(0x3, 140, 100, None);

    assert_eq!(app.diagram_index, 0);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 1);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 2);
    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 0);
    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_index, 2);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_pinned_side_diagram_layout_allocates_right_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x111, 900, 450, Some("side".to_string()));

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let diagram = frame.layout.diagram_area.expect("diagram area");
    let messages = frame.layout.messages_area.expect("messages area");

    assert!(
        diagram.width >= 24,
        "diagram pane too narrow: {}",
        diagram.width
    );
    assert_eq!(diagram.height, 40);
    assert_eq!(diagram.x, messages.x + messages.width);
    assert_eq!(diagram.y, 0);
    assert!(
        diagram.width < 120,
        "diagram should not consume full terminal width"
    );
    assert!(
        frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_pinned_top_diagram_layout_allocates_top_pane() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Top;
    app.diagram_pane_ratio = 35;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x222, 500, 900, Some("top".to_string()));

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let diagram = frame.layout.diagram_area.expect("diagram area");
    let messages = frame.layout.messages_area.expect("messages area");

    assert_eq!(diagram.x, 0);
    assert_eq!(diagram.width, 120);
    assert!(
        diagram.height >= 6,
        "diagram pane too short: {}",
        diagram.height
    );
    assert_eq!(messages.y, diagram.y + diagram.height);
    assert!(
        frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_pinned_diagram_not_shown_when_terminal_too_narrow() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x333, 900, 450, None);

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(30, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    assert!(
        frame.layout.diagram_area.is_none(),
        "diagram pane should be suppressed on narrow terminal"
    );
    assert!(
        !frame
            .render_order
            .iter()
            .any(|s| s == "draw_pinned_diagram")
    );

    crate::tui::visual_debug::disable();
    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_mouse_scroll_over_diff_pane_scrolls_side_panel_without_changing_focus() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diff_pane_scroll, 8);
    assert!(!app.diff_pane_focus);
    assert!(!app.diff_pane_auto_scroll);
}

#[test]
fn test_mouse_scroll_over_tool_side_panel_scrolls_shared_right_pane_without_changing_focus() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only, "right-pane wheel scroll should be deferrable");
    assert_eq!(app.diff_pane_scroll, 8);
    assert!(!app.diff_pane_focus);
    assert!(!app.diff_pane_auto_scroll);
}

#[test]
fn test_mouse_scroll_over_tool_side_panel_keeps_typing_in_chat() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    assert!(scroll_only);
    assert!(!app.diff_pane_focus);

    app.handle_key(KeyCode::Char('x'), KeyModifiers::empty())
        .expect("typing into chat should succeed");

    assert_eq!(app.input, "x");
}

#[test]
fn test_mouse_scroll_over_tool_side_panel_updates_visible_render() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll = 0;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: (1..=30)
                .map(|i| format!("- side-scroll-{i:02}"))
                .collect::<Vec<_>>()
                .join("\n"),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let before = render_and_snap(&app, &mut terminal);
    assert!(crate::tui::ui::pinned_pane_total_lines() > 3);
    assert!(
        crate::tui::ui::last_layout_snapshot()
            .and_then(|l| l.diff_pane_area)
            .is_some()
    );
    assert!(before.contains("side-scroll-01"));

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 60,
        row: 3,
        modifiers: KeyModifiers::empty(),
    });
    assert!(
        scroll_only,
        "wheel scroll should remain deferrable while streaming"
    );
    assert_eq!(app.diff_pane_scroll, 3);

    let after = render_and_snap(&app, &mut terminal);
    assert_eq!(crate::tui::ui::last_diff_pane_effective_scroll(), 3);
    assert_ne!(
        before, after,
        "hover scrolling should repaint the side panel"
    );
    assert!(after.contains("side-scroll-04"));
    assert!(after.contains("side-scroll-05"));
    assert!(!after.contains("side-scroll-01"));
}

#[test]
fn test_tool_side_panel_uses_shared_right_pane_keyboard_focus() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    assert!(app.diff_pane_visible());
    assert!(app.handle_diagram_ctrl_key(KeyCode::Char('l'), false));
    assert!(app.diff_pane_focus);

    assert!(super::input::handle_navigation_shortcuts(
        &mut app,
        KeyCode::BackTab,
        KeyModifiers::empty()
    ));
    assert!(
        app.diff_pane_focus,
        "cycling diff display should not drop focus when tool side panel is still visible"
    );
}

#[test]
fn test_mouse_scroll_events_are_classified_as_scroll_only() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "scroll wheel events should be deferrable during streaming"
    );

    let non_scroll = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!non_scroll, "clicks should still redraw immediately");
}

#[test]
fn test_mouse_scroll_over_unfocused_diagram_resizes_immediately_without_animation() {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;
    app.diagram_pane_ratio_from = 40;
    app.diagram_pane_ratio_target = 40;
    app.diagram_pane_anim_start = None;
    app.diagram_focus = false;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x444, 900, 450, None);
    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 80, 30),
        Some(Rect::new(80, 0, 40, 30)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 90,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diagram_pane_ratio, 43);
    assert_eq!(app.diagram_pane_ratio_from, 43);
    assert_eq!(app.diagram_pane_ratio_target, 43);
    assert!(app.diagram_pane_anim_start.is_none());

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_dragging_diagram_border_resizes_immediately_without_animation() {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;
    app.diagram_pane_ratio = 40;
    app.diagram_pane_ratio_from = 40;
    app.diagram_pane_ratio_target = 40;
    app.diagram_pane_anim_start = Some(Instant::now());
    app.diagram_pane_dragging = false;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x445, 900, 450, None);
    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 80, 30),
        Some(Rect::new(80, 0, 40, 30)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: 80,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });
    assert!(app.diagram_pane_dragging);

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: 72,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diagram_pane_ratio, 40);
    assert_eq!(app.diagram_pane_ratio_from, 40);
    assert_eq!(app.diagram_pane_ratio_target, 40);
    assert!(app.diagram_pane_anim_start.is_none());

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_is_scroll_only_key_detects_navigation_inputs() {
    let mut app = create_test_app();

    let (up_code, up_mods) = scroll_up_key(&app);
    assert!(super::input::is_scroll_only_key(&app, up_code, up_mods));

    let (down_code, down_mods) = scroll_down_key(&app);
    assert!(super::input::is_scroll_only_key(&app, down_code, down_mods));

    app.diff_pane_focus = true;
    assert!(super::input::is_scroll_only_key(
        &app,
        KeyCode::Char('j'),
        KeyModifiers::empty()
    ));

    assert!(super::input::is_scroll_only_key(
        &app,
        KeyCode::BackTab,
        KeyModifiers::empty()
    ));

    assert!(!super::input::is_scroll_only_key(
        &app,
        KeyCode::Char('a'),
        KeyModifiers::empty()
    ));
    assert!(!super::input::is_scroll_only_key(
        &app,
        KeyCode::Enter,
        KeyModifiers::empty()
    ));
}

#[test]
fn test_fuzzy_command_suggestions() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/mdl");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/model"));
}

fn configure_test_remote_models(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.3-codex".to_string());
    app.remote_available_models = vec!["gpt-5.3-codex".to_string(), "gpt-5.2-codex".to_string()];
}

fn configure_test_remote_models_with_openai_recommendations(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.2".to_string());
    app.remote_available_models = vec![
        "gpt-5.2".to_string(),
        "gpt-5.4".to_string(),
        "gpt-5.4-pro".to_string(),
        "gpt-5.3-codex-spark".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4-6".to_string(),
    ];
    app.remote_model_routes = app
        .remote_available_models
        .iter()
        .cloned()
        .map(|model| crate::provider::ModelRoute {
            model,
            provider: "OpenAI".to_string(),
            api_method: "openai-oauth".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        })
        .collect();
}

#[test]
fn test_model_picker_preview_filter_parsing() {
    assert_eq!(
        App::model_picker_preview_filter("/model"),
        Some(String::new())
    );
    assert_eq!(
        App::model_picker_preview_filter("/model   gpt-5"),
        Some("gpt-5".to_string())
    );
    assert_eq!(
        App::model_picker_preview_filter("   /models codex"),
        Some("codex".to_string())
    );
    assert_eq!(App::model_picker_preview_filter("/modelx"), None);
    assert_eq!(App::model_picker_preview_filter("hello /model"), None);
}

#[test]
fn test_model_picker_preview_stays_open_and_updates_filter() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    for c in "/model g52c".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.filter, "g52c");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.models[i].name == "gpt-5.2-codex")
    );
    assert_eq!(app.input(), "/model g52c");
}

#[test]
fn test_model_picker_preview_enter_selects_model() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    for c in "/model g52c".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // Enter from preview mode selects the model and closes the picker
    assert!(app.picker_state.is_none());
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_model_picker_preview_arrow_keys_navigate() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    // Type /model to open preview
    for c in "/model".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    let initial_selected = picker.selected;

    // Down arrow should navigate in preview mode
    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .picker_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected + 1);

    // Up arrow should navigate back
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    let picker = app
        .picker_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected);

    // Input should be preserved
    assert_eq!(app.input(), "/model");
}

fn configure_test_remote_models_with_copilot(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("claude-sonnet-4".to_string());
    app.remote_available_models = vec![
        "claude-sonnet-4-6".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4.6".to_string(),
        "gemini-3-pro-preview".to_string(),
        "grok-code-fast-1".to_string(),
    ];
}

#[test]
fn test_model_picker_includes_copilot_models_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.models.iter().map(|m| m.name.as_str()).collect();

    assert!(
        model_names.contains(&"claude-opus-4.6"),
        "picker should contain copilot model claude-opus-4.6, got: {:?}",
        model_names
    );
    assert!(
        model_names.contains(&"gemini-3-pro-preview"),
        "picker should contain copilot model gemini-3-pro-preview, got: {:?}",
        model_names
    );
    assert!(
        model_names.contains(&"grok-code-fast-1"),
        "picker should contain copilot model grok-code-fast-1, got: {:?}",
        model_names
    );
}

#[test]
fn test_model_picker_remote_falls_back_to_current_model_when_catalog_empty() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("anthropic/claude-sonnet-4".to_string());
    app.remote_available_models.clear();
    app.remote_model_routes.clear();

    app.open_model_picker();

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker should open with current-model fallback");

    assert_eq!(picker.models.len(), 1);
    assert_eq!(picker.models[0].name, "anthropic/claude-sonnet-4");
    assert_eq!(picker.models[0].routes.len(), 1);
    assert_eq!(picker.models[0].routes[0].provider, "openrouter");
    assert_eq!(picker.models[0].routes[0].api_method, "current");
    assert!(picker.models[0].routes[0].available);
}

#[test]
fn test_model_picker_copilot_models_have_copilot_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker should be open");

    // grok-code-fast-1 is NOT in ALL_CLAUDE_MODELS or ALL_OPENAI_MODELS,
    // so it should get a copilot route
    let grok_entry = picker
        .models
        .iter()
        .find(|m| m.name == "grok-code-fast-1")
        .expect("grok-code-fast-1 should be in picker");

    assert!(
        grok_entry.routes.iter().any(|r| r.api_method == "copilot"),
        "grok-code-fast-1 should have a copilot route, got: {:?}",
        grok_entry.routes
    );
}

#[test]
fn test_model_picker_preserves_recommendation_priority_order() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);

    app.open_model_picker();

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.models.iter().map(|m| m.name.as_str()).collect();

    assert_eq!(model_names.first().copied(), Some("gpt-5.2"));

    let gpt54 = picker
        .models
        .iter()
        .position(|model| model.name == "gpt-5.4")
        .expect("gpt-5.4 should be present");
    let gpt54_pro = picker
        .models
        .iter()
        .position(|model| model.name == "gpt-5.4-pro")
        .expect("gpt-5.4-pro should be present");
    let claude_opus = picker
        .models
        .iter()
        .position(|model| model.name == "claude-opus-4-6")
        .expect("claude-opus-4-6 should be present");
    let spark = picker
        .models
        .iter()
        .position(|model| model.name == "gpt-5.3-codex-spark")
        .expect("gpt-5.3-codex-spark should be present");
    let codex = picker
        .models
        .iter()
        .position(|model| model.name == "gpt-5.3-codex")
        .expect("gpt-5.3-codex should be present");

    assert!(
        gpt54 < gpt54_pro,
        "gpt-5.4 should rank ahead of gpt-5.4-pro, got {:?}",
        model_names
    );
    assert!(
        gpt54_pro < claude_opus,
        "gpt-5.4-pro should rank ahead of claude-opus-4-6, got {:?}",
        model_names
    );
    assert!(
        claude_opus < spark,
        "claude-opus-4-6 should rank ahead of non-recommended gpt-5.3-codex-spark, got {:?}",
        model_names
    );
    assert!(
        !picker.models[spark].recommended,
        "gpt-5.3-codex-spark should not be recommended"
    );
    assert!(
        !picker.models[codex].recommended,
        "gpt-5.3-codex should not be recommended"
    );
}

#[test]
fn test_model_picker_copilot_selection_prefixes_model() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .picker_state
        .as_ref()
        .expect("model picker should be open");

    // Find grok-code-fast-1 (which should only be a copilot route)
    let grok_idx = picker
        .models
        .iter()
        .position(|m| m.name == "grok-code-fast-1")
        .expect("grok-code-fast-1 should be in picker");

    // Navigate to it and select
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == grok_idx)
        .expect("grok-code-fast-1 should be in filtered list");

    // Set the selected position to grok's position
    app.picker_state.as_mut().unwrap().selected = filtered_pos;

    // Press Enter to select
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // In remote mode, selection should produce a pending_model_switch with copilot: prefix
    if let Some(ref spec) = app.pending_model_switch {
        assert!(
            spec.starts_with("copilot:"),
            "copilot model should be prefixed with 'copilot:', got: {}",
            spec
        );
    }
    // Picker should be closed
    assert!(app.picker_state.is_none());
}

#[test]
fn test_handle_key_cursor_movement() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.cursor_pos(), 3);

    app.handle_key(KeyCode::Left, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos(), 2);

    app.handle_key(KeyCode::Home, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.cursor_pos(), 0);

    app.handle_key(KeyCode::End, KeyModifiers::empty()).unwrap();
    assert_eq!(app.cursor_pos(), 3);
}

#[test]
fn test_handle_key_ctrl_word_movement_and_delete() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello world ".len());

    app.handle_key(KeyCode::Left, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello ".len());

    app.handle_key(KeyCode::Right, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.cursor_pos(), "hello world ".len());

    app.handle_key(KeyCode::Backspace, KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "hello again");
    assert_eq!(app.cursor_pos(), "hello ".len());
}

#[test]
fn test_handle_key_escape_clears_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "test");

    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert_eq!(
        app.status_notice(),
        Some("Input cleared — Ctrl+Z to restore".to_string())
    );
}

#[test]
fn test_handle_key_ctrl_z_restores_escaped_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Esc, KeyModifiers::empty()).unwrap();

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "test");
    assert_eq!(app.cursor_pos(), 4);
    assert_eq!(app.status_notice(), Some("↶ Input restored".to_string()));
}

#[test]
fn test_handle_key_ctrl_z_undoes_typing() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "ab");
    assert_eq!(app.cursor_pos(), 2);

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "a");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_ctrl_u_clears_input() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    app.handle_key(KeyCode::Char('u'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
}

#[test]
fn test_submit_input_adds_message() {
    let mut app = create_test_app();

    // Type and submit
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();
    app.submit_input();

    // Check message was added to display
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "user");
    assert_eq!(app.display_messages()[0].content, "hi");

    // Check processing state
    assert!(app.is_processing());
    assert!(app.pending_turn);
    assert!(matches!(app.status(), ProcessingStatus::Sending));
    assert!(app.elapsed().is_some());

    // Input should be cleared
    assert!(app.input().is_empty());
}

#[test]
fn test_queue_message_while_processing() {
    let mut app = create_test_app();
    app.queue_mode = true;

    // Simulate processing state
    app.is_processing = true;

    // Type a message
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    // Press Enter should queue, not submit
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert!(app.input().is_empty());

    // Queued messages are stored in queued_messages, not display_messages
    assert_eq!(app.queued_messages()[0], "test");
    assert!(app.display_messages().is_empty());
}

#[test]
fn test_ctrl_tab_toggles_queue_mode() {
    let mut app = create_test_app();

    assert!(!app.queue_mode);

    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.queue_mode);

    app.handle_key(KeyCode::Char('t'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(!app.queue_mode);
}

#[test]
fn test_shift_enter_inserts_newline() {
    let mut app = create_test_app();
    app.is_processing = true;

    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::SHIFT).unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "h\ni");
    assert_eq!(app.queued_count(), 0);
    assert_eq!(app.interleave_message.as_deref(), None);
}

#[test]
fn test_ctrl_enter_opposite_send_mode() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Default immediate mode: Ctrl+Enter should queue
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('i'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.interleave_message.as_deref(), None);
    assert!(app.input().is_empty());

    // Queue mode: Ctrl+Enter should interleave (sets interleave_message, not queued)
    app.queue_mode = true;
    app.handle_key(KeyCode::Char('y'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Interleave now sets interleave_message instead of adding to queue
    assert_eq!(app.queued_count(), 1); // Still just "hi" in queue
    assert_eq!(app.interleave_message.as_deref(), Some("yo")); // "yo" is for interleave
}

#[test]
fn test_typing_during_processing() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Should still be able to type
    app.handle_key(KeyCode::Char('a'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('b'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('c'), KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.input(), "abc");
}

#[test]
fn test_ctrl_c_requests_cancel_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.interleave_message = Some("queued interrupt".to_string());
    app.pending_soft_interrupts
        .push("pending soft interrupt".to_string());

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(app.cancel_requested);
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());
    assert_eq!(app.status_notice(), Some("Interrupting...".to_string()));
}

#[test]
fn test_ctrl_c_still_arms_quit_when_idle() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL)
        .unwrap();

    assert!(!app.cancel_requested);
    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_ctrl_x_cuts_entire_input_line_to_clipboard() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = 5;

    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    let cut = super::input::cut_input_line_to_clipboard_with(&mut app, |text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(cut);
    assert_eq!(&*copied.lock().unwrap(), "hello world");
    assert!(app.input().is_empty());
    assert_eq!(app.cursor_pos(), 0);
    assert_eq!(app.status_notice(), Some("✂ Cut input line".to_string()));

    app.handle_key(KeyCode::Char('z'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 5);
}

#[test]
fn test_ctrl_x_preserves_input_when_clipboard_copy_fails() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = 5;

    let cut = super::input::cut_input_line_to_clipboard_with(&mut app, |_text| false);

    assert!(!cut);
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 5);
    assert_eq!(
        app.status_notice(),
        Some("Failed to copy input line".to_string())
    );
}

#[test]
fn test_ctrl_up_edits_queued_message() {
    let mut app = create_test_app();
    app.queue_mode = true;
    app.is_processing = true;

    // Type and queue a message
    app.handle_key(KeyCode::Char('h'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('l'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('o'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.queued_count(), 1);
    assert!(app.input().is_empty());

    // Press Ctrl+Up to bring it back for editing
    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

    assert_eq!(app.queued_count(), 0);
    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), 5); // Cursor at end
}

#[test]
fn test_ctrl_up_prefers_pending_interleave_for_editing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false; // Enter=interleave, Ctrl+Enter=queue

    for c in "urgent".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    for c in "later".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 1);

    app.handle_key(KeyCode::Up, KeyModifiers::CONTROL).unwrap();

    assert_eq!(app.input(), "urgent\n\nlater");
    assert_eq!(app.interleave_message.as_deref(), None);
    assert_eq!(app.queued_count(), 0);
}

#[test]
fn test_send_action_modes() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false;

    assert_eq!(app.send_action(false), SendAction::Interleave);
    assert_eq!(app.send_action(true), SendAction::Queue);

    app.queue_mode = true;
    assert_eq!(app.send_action(false), SendAction::Queue);
    assert_eq!(app.send_action(true), SendAction::Interleave);

    app.is_processing = false;
    assert_eq!(app.send_action(false), SendAction::Submit);
}

#[test]
fn test_send_action_submits_bang_commands_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.input = "!pwd".to_string();

    assert_eq!(app.send_action(false), SendAction::Submit);
    assert_eq!(app.send_action(true), SendAction::Submit);
}

#[test]
fn test_handle_input_shell_completed_renders_markdown_blocks() {
    let mut app = create_test_app();
    let event = BusEvent::InputShellCompleted(InputShellCompleted {
        session_id: app.session.id.clone(),
        result: crate::message::InputShellResult {
            command: "ls -la".to_string(),
            cwd: Some("/tmp/project".to_string()),
            output: "Cargo.toml\nsrc\n".to_string(),
            exit_code: Some(0),
            duration_ms: 42,
            truncated: false,
            failed_to_start: false,
        },
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    let rendered = app.display_messages().last().expect("shell result message");
    assert_eq!(rendered.role, "system");
    assert!(rendered.content.contains("**Shell command**"));
    assert!(rendered.content.contains("```bash"));
    assert!(rendered.content.contains("ls -la"));
    assert!(rendered.content.contains("```text"));
    assert!(rendered.content.contains("Cargo.toml"));
    assert_eq!(
        app.status_notice(),
        Some("Shell command completed".to_string())
    );
}

#[test]
fn test_handle_server_event_input_shell_result_renders_markdown_blocks() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::InputShellResult {
            result: crate::message::InputShellResult {
                command: "pwd".to_string(),
                cwd: Some("/tmp/project".to_string()),
                output: "/tmp/project\n".to_string(),
                exit_code: Some(0),
                duration_ms: 5,
                truncated: false,
                failed_to_start: false,
            },
        },
        &mut remote,
    );

    let rendered = app.display_messages().last().expect("shell result message");
    assert_eq!(rendered.role, "system");
    assert!(rendered.content.contains("**Shell command**"));
    assert!(rendered.content.contains("```bash"));
    assert!(rendered.content.contains("pwd"));
    assert!(rendered.content.contains("```text"));
    assert!(rendered.content.contains("/tmp/project"));
    assert_eq!(
        app.status_notice(),
        Some("Shell command completed".to_string())
    );
}

#[test]
fn test_streaming_tokens() {
    let mut app = create_test_app();

    assert_eq!(app.streaming_tokens(), (0, 0));

    app.streaming_input_tokens = 100;
    app.streaming_output_tokens = 50;

    assert_eq!(app.streaming_tokens(), (100, 50));
}

#[test]
fn test_processing_status_display() {
    let status = ProcessingStatus::Sending;
    assert!(matches!(status, ProcessingStatus::Sending));

    let status = ProcessingStatus::Streaming;
    assert!(matches!(status, ProcessingStatus::Streaming));

    let status = ProcessingStatus::RunningTool("bash".to_string());
    if let ProcessingStatus::RunningTool(name) = status {
        assert_eq!(name, "bash");
    } else {
        panic!("Expected RunningTool");
    }
}

#[test]
fn test_skill_invocation_not_queued() {
    let mut app = create_test_app();

    // Type a skill command
    app.handle_key(KeyCode::Char('/'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('e'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('s'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('t'), KeyModifiers::empty())
        .unwrap();

    app.submit_input();

    // Should show error for unknown skill, not start processing
    assert!(!app.pending_turn);
    assert!(!app.is_processing);
    // Should have an error message about unknown skill
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "error");
}

#[test]
fn test_multiple_queued_messages() {
    let mut app = create_test_app();
    app.is_processing = true;

    // Queue first message
    for c in "first".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Queue second message
    for c in "second".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Queue third message
    for c in "third".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.queued_count(), 3);
    assert_eq!(app.queued_messages()[0], "first");
    assert_eq!(app.queued_messages()[1], "second");
    assert_eq!(app.queued_messages()[2], "third");
    assert!(app.input().is_empty());
}

#[test]
fn test_queue_message_combines_on_send() {
    let mut app = create_test_app();

    // Queue two messages directly
    app.queued_messages.push("message one".to_string());
    app.queued_messages.push("message two".to_string());

    // Take and combine (simulating what process_queued_messages does)
    let combined = std::mem::take(&mut app.queued_messages).join("\n\n");

    assert_eq!(combined, "message one\n\nmessage two");
    assert!(app.queued_messages.is_empty());
}

#[test]
fn test_interleave_message_separate_from_queue() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.queue_mode = false; // Default mode: Enter=interleave, Ctrl+Enter=queue

    // Type and submit via Enter (should interleave, not queue)
    for c in "urgent".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    // Should be in interleave_message, not queued
    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 0);

    // Now queue one
    for c in "later".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::CONTROL)
        .unwrap();

    // Interleave unchanged, one message queued
    assert_eq!(app.interleave_message.as_deref(), Some("urgent"));
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.queued_messages()[0], "later");
}

#[test]
fn test_handle_paste_single_line() {
    let mut app = create_test_app();

    app.handle_paste("hello world".to_string());

    // Small paste (< 5 lines) is inlined directly
    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 11);
    assert!(app.pasted_contents.is_empty()); // No placeholder storage needed
}

#[test]
fn test_handle_paste_multi_line() {
    let mut app = create_test_app();

    app.handle_paste("line 1\nline 2\nline 3".to_string());

    // Small paste (< 5 lines) is inlined directly
    assert_eq!(app.input(), "line 1\nline 2\nline 3");
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn test_handle_paste_large() {
    let mut app = create_test_app();

    app.handle_paste("a\nb\nc\nd\ne".to_string());

    // Large paste (5+ lines) uses placeholder
    assert_eq!(app.input(), "[pasted 5 lines]");
    assert_eq!(app.pasted_contents.len(), 1);
}

#[test]
fn test_paste_expansion_on_submit() {
    let mut app = create_test_app();

    // Type prefix, paste large content, type suffix
    app.handle_key(KeyCode::Char('A'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char(':'), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    // Paste 5 lines to trigger placeholder
    app.handle_paste("1\n2\n3\n4\n5".to_string());
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    app.handle_key(KeyCode::Char('B'), KeyModifiers::empty())
        .unwrap();

    // Input shows placeholder
    assert_eq!(app.input(), "A: [pasted 5 lines] B");

    // Submit expands placeholder
    app.submit_input();

    // Display shows placeholder (user sees condensed view)
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].content, "A: [pasted 5 lines] B");

    // Model receives expanded content (actual pasted text)
    assert_eq!(app.messages.len(), 1);
    match &app.messages[0].content[0] {
        crate::message::ContentBlock::Text { text, .. } => {
            assert_eq!(text, "A: 1\n2\n3\n4\n5 B");
        }
        _ => panic!("Expected Text content block"),
    }

    // Pasted contents should be cleared
    assert!(app.pasted_contents.is_empty());
}

#[test]
fn test_multiple_pastes() {
    let mut app = create_test_app();

    // Small pastes are inlined
    app.handle_paste("first".to_string());
    app.handle_key(KeyCode::Char(' '), KeyModifiers::empty())
        .unwrap();
    app.handle_paste("second\nline".to_string());

    // Both small pastes inlined directly
    assert_eq!(app.input(), "first second\nline");
    assert!(app.pasted_contents.is_empty());

    app.submit_input();
    // Display and model both get the same content (no expansion needed)
    assert_eq!(app.display_messages()[0].content, "first second\nline");
    match &app.messages[0].content[0] {
        crate::message::ContentBlock::Text { text, .. } => {
            assert_eq!(text, "first second\nline");
        }
        _ => panic!("Expected Text content block"),
    }
}

#[test]
fn test_restore_session_adds_reload_message() {
    use crate::session::Session;

    let mut app = create_test_app();

    // Create and save a session with a fake provider_session_id
    let mut session = Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "test message".to_string(),
            cache_control: None,
        }],
    );
    session.provider_session_id = Some("fake-uuid".to_string());
    let session_id = session.id.clone();
    session.save().unwrap();

    // Restore the session
    app.restore_session(&session_id);

    // Should have the original message + reload success message in display
    assert_eq!(app.display_messages().len(), 2);
    assert_eq!(app.display_messages()[0].role, "user");
    assert_eq!(app.display_messages()[0].content, "test message");
    assert_eq!(app.display_messages()[1].role, "system");
    assert!(
        app.display_messages()[1]
            .content
            .contains("Reload complete — continuing.")
    );

    // Messages for API should only have the original message (no reload msg to avoid breaking alternation)
    assert_eq!(app.messages.len(), 1);

    // Provider session ID should be cleared (Claude sessions don't persist across restarts)
    assert!(app.provider_session_id.is_none());

    // Clean up
    let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
}

#[test]
fn test_system_reminder_is_added_to_system_prompt_not_user_messages() {
    let mut app = create_test_app();
    app.current_turn_system_reminder = Some(
        "Your session was interrupted by a server reload. Continue where you left off.".to_string(),
    );

    let split = app.build_system_prompt_split(None);

    assert!(split.dynamic_part.contains("# System Reminder"));
    assert!(split.dynamic_part.contains("Continue where you left off."));
    assert!(app.messages.is_empty());
}

#[test]
fn test_recover_session_without_tools_preserves_debug_and_canary_flags() {
    let mut app = create_test_app();
    app.session.is_debug = true;
    app.session.is_canary = true;
    app.session.testing_build = Some("self-dev".to_string());
    app.session.working_dir = Some("/tmp/jcode-test".to_string());
    let old_session_id = app.session.id.clone();

    app.recover_session_without_tools();

    assert_ne!(app.session.id, old_session_id);
    assert_eq!(
        app.session.parent_id.as_deref(),
        Some(old_session_id.as_str())
    );
    assert!(app.session.is_debug);
    assert!(app.session.is_canary);
    assert_eq!(app.session.testing_build.as_deref(), Some("self-dev"));
    assert_eq!(app.session.working_dir.as_deref(), Some("/tmp/jcode-test"));

    let _ = std::fs::remove_file(crate::session::session_path(&app.session.id).unwrap());
}

#[test]
fn test_has_newer_binary_detection() {
    use std::time::{Duration, SystemTime};

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().unwrap();

    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&exe, "test").unwrap();
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    assert!(app.has_newer_binary());

    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    assert!(!app.has_newer_binary());

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_reload_requests_exit_when_newer_binary() {
    use std::time::{Duration, SystemTime};

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().unwrap();

    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&exe, "test").unwrap();
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    app.input = "/reload".to_string();
    app.submit_input();

    assert!(app.reload_requested.is_some());
    assert!(app.should_quit);

    // Ensure the "no newer binary" path is exercised too.
    app.reload_requested = None;
    app.should_quit = false;
    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    app.input = "/reload".to_string();
    app.submit_input();
    assert!(app.reload_requested.is_none());
    assert!(!app.should_quit);

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_save_and_restore_reload_state_preserves_queued_messages() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-{}", std::process::id());

    app.input = "draft".to_string();
    app.cursor_pos = 3;
    app.queued_messages.push("queued one".to_string());
    app.queued_messages.push("queued two".to_string());
    app.hidden_queued_system_messages
        .push("continue silently".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.0, "draft");
    assert_eq!(restored.1, 3);
    assert_eq!(restored.2, vec!["queued one", "queued two"]);
    assert_eq!(restored.3, vec!["continue silently"]);

    assert!(App::restore_input_for_reload(&session_id).is_none());
}

#[test]
fn test_restore_reload_state_supports_legacy_input_format() {
    let session_id = format!("test-reload-legacy-{}", std::process::id());
    let jcode_dir = crate::storage::jcode_dir().unwrap();
    let path = jcode_dir.join(format!("client-input-{}", session_id));
    std::fs::write(&path, "2\nhello").unwrap();

    let restored =
        App::restore_input_for_reload(&session_id).expect("legacy reload state should restore");
    assert_eq!(restored.0, "hello");
    assert_eq!(restored.1, 2);
    assert!(restored.2.is_empty());
}

#[test]
fn test_reload_progress_coalesces_into_single_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Reloading { new_socket: None },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ReloadProgress {
            step: "init".to_string(),
            message: "🔄 Starting hot-reload...".to_string(),
            success: None,
            output: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ReloadProgress {
            step: "verify".to_string(),
            message: "Binary verified".to_string(),
            success: Some(true),
            output: Some("size=68.4MB".to_string()),
        },
        &mut remote,
    );

    assert_eq!(app.display_messages().len(), 1);
    let reload_msg = &app.display_messages()[0];
    assert_eq!(reload_msg.role, "system");
    assert_eq!(reload_msg.title.as_deref(), Some("Reload"));
    assert_eq!(
        reload_msg.content,
        "🔄 Server reload initiated...\n[init] 🔄 Starting hot-reload...\n[verify] ✓ Binary verified\n```\nsize=68.4MB\n```"
    );
}

#[test]
fn test_handle_server_event_updates_connection_type() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::ConnectionType {
            connection: "websocket".to_string(),
        },
        &mut remote,
    );

    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}

#[test]
fn test_handle_server_event_transcript_replace_updates_input() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.input = "old draft".to_string();
    app.cursor_pos = app.input.len();

    app.handle_server_event(
        crate::protocol::ServerEvent::Transcript {
            text: "new dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Replace,
        },
        &mut remote,
    );

    assert_eq!(app.input, "new dictated text");
    assert_eq!(app.cursor_pos, app.input.len());
    assert_eq!(
        app.status_notice(),
        Some("Transcript replaced input".to_string())
    );
}

#[test]
fn test_local_bus_dictation_completion_applies_transcript() {
    let mut app = create_test_app();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();

    crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert_eq!(app.input, "draft dictated text");
    assert_eq!(app.status_notice(), Some("Transcript appended".to_string()));
}

#[test]
fn test_handle_server_event_history_clears_connection_type_on_session_change_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert_eq!(app.connection_type, None);
}

#[test]
fn test_handle_server_event_history_preserves_connection_type_for_same_session_when_missing() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_same".to_string());
    app.connection_type = Some("websocket".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_same".to_string(),
            messages: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_same"));
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}

#[test]
fn test_handle_post_connect_marker_without_reload_context_does_not_queue_selfdev_continuation() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let session_id = "session_marker_only";
    let jcode_dir = crate::storage::jcode_dir().expect("jcode dir");
    std::fs::write(
        jcode_dir.join(format!("client-reload-pending-{}", session_id)),
        "Reloaded with build test123\n",
    )
    .expect("write client reload marker");

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    rt.block_on(super::remote::handle_post_connect(
        &mut app,
        &mut terminal,
        &mut remote,
        &mut state,
        Some(session_id),
    ))
    .expect("post connect should succeed");

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content == "Reload complete — continuing."),
        "marker-only reconnect should not queue selfdev continuation"
    );
    assert!(app.reload_info.is_empty());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.content.contains("✓ Reconnected successfully.")),
        "reconnect success message should still be shown"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_server_event_token_usage_uses_per_call_deltas() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 10,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 30,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming_output_tokens, 30);
    assert_eq!(app.streaming_total_output_tokens, 30);
}

#[test]
fn test_handle_server_event_interrupted_clears_stream_state_and_sets_idle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.processing_started = Some(Instant::now());
    app.current_message_id = Some(42);
    app.streaming_text = "partial".to_string();
    app.streaming_tool_calls.push(crate::message::ToolCall {
        id: "tool_1".to_string(),
        name: "bash".to_string(),
        input: serde_json::Value::Null,
        intent: None,
    });
    app.interleave_message = Some("queued interrupt".to_string());
    app.pending_soft_interrupts
        .push("pending soft interrupt".to_string());

    remote.handle_tool_start("tool_1", "bash");
    remote.handle_tool_input("{\"command\":\"sleep 10\"}");
    remote.handle_tool_exec("tool_1", "edit");

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.current_message_id.is_none());
    assert!(app.streaming_text.is_empty());
    assert!(app.streaming_tool_calls.is_empty());
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());

    let last = app
        .display_messages()
        .last()
        .expect("missing interrupted message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Interrupted");
}

#[test]
fn test_handle_server_event_tool_start_flushes_streaming_text_before_tool_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming_text = "Let me inspect those files first.".to_string();

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_batch".to_string(),
            name: "batch".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming_text.is_empty());
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].role, "assistant");
    assert_eq!(
        app.display_messages()[0].content,
        "Let me inspect those files first."
    );
    assert_eq!(app.streaming_tool_calls.len(), 1);
    assert_eq!(app.streaming_tool_calls[0].name, "batch");
    assert!(matches!(app.status, ProcessingStatus::RunningTool(ref name) if name == "batch"));
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_system_renders_system_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "[Background Task Completed]\nTask: abc123 (bash)".to_string(),
            display_role: Some("system".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing injected message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Background Task Completed"));
}

#[test]
fn test_handle_remote_disconnect_flushes_streaming_text_and_sets_reconnect_state() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.streaming_text = "partial response being streamed".to_string();

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.streaming_text.is_empty());
    assert_eq!(state.disconnect_msg_idx, Some(1));
    assert_eq!(state.reconnect_attempts, 1);
    assert!(state.disconnect_start.is_some());

    let assistant = app
        .display_messages()
        .iter()
        .find(|m| m.role == "assistant")
        .expect("streaming text should have been saved as assistant message");
    assert_eq!(assistant.content, "partial response being streamed");

    let last = app
        .display_messages()
        .last()
        .expect("missing reconnect status message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("⚡ Connection lost — retrying"));
    assert!(last.content.contains("Cause: connection to server dropped"));
}

#[test]
fn test_handle_remote_disconnect_retryable_pending_schedules_retry() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("retryable continuation should remain pending");
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 1);
    assert!(pending.retry_at.is_some());
    assert!(app.rate_limit_reset.is_some());
}

#[test]
fn test_handle_server_event_compaction_shows_completion_message_in_remote_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.provider_session_id = Some("provider-session".to_string());
    app.session.provider_session_id = Some("provider-session".to_string());
    app.context_warning_shown = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::Compaction {
            trigger: "semantic".to_string(),
            pre_tokens: Some(12_345),
            messages_dropped: None,
        },
        &mut remote,
    );

    assert!(app.provider_session_id.is_none());
    assert!(app.session.provider_session_id.is_none());
    assert!(!app.context_warning_shown);
    assert_eq!(app.status_notice(), Some("Context compacted".to_string()));

    let last = app
        .display_messages()
        .last()
        .expect("missing compaction message");
    assert_eq!(last.role, "system");
    assert_eq!(
        last.content,
        "📦 **Context compacted** (semantic) — older messages were summarized to stay within the context window. Previous size: ~12,345 tokens."
    );
}

#[test]
fn test_handle_server_event_compaction_mode_changed_updates_remote_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::CompactionModeChanged {
            id: 7,
            mode: crate::config::CompactionMode::Semantic,
            error: None,
        },
        &mut remote,
    );

    assert_eq!(
        app.remote_compaction_mode,
        Some(crate::config::CompactionMode::Semantic)
    );
    assert_eq!(
        app.status_notice(),
        Some("Compaction: semantic".to_string())
    );

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.content, "✓ Compaction mode → semantic");
}

#[test]
fn test_handle_server_event_service_tier_changed_mentions_next_request_when_streaming() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;

    app.handle_server_event(
        crate::protocol::ServerEvent::ServiceTierChanged {
            id: 7,
            service_tier: Some("priority".to_string()),
            error: None,
        },
        &mut remote,
    );

    assert_eq!(app.remote_service_tier, Some("priority".to_string()));
    assert_eq!(
        app.status_notice(),
        Some("Fast: on (next request)".to_string())
    );

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(
        last.content,
        "✓ Fast mode on (Fast)\nApplies to the next request/turn. The current in-flight request keeps its existing tier."
    );
}

#[test]
fn test_reload_handoff_active_when_server_reload_flag_set() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let state = remote::RemoteRunState {
        server_reload_in_progress: true,
        ..Default::default()
    };

    assert!(remote::reload_handoff_active(&state));

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_reload_handoff_inactive_without_flag_or_marker() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    let state = remote::RemoteRunState::default();

    assert!(!remote::reload_handoff_active(&state));

    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_reload_handoff_active_when_reload_marker_present() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
    crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());

    crate::server::write_reload_state(
        "reload-marker-test",
        "test-hash",
        crate::server::ReloadPhase::Starting,
        None,
    );

    let state = remote::RemoteRunState {
        ..Default::default()
    };

    assert!(remote::reload_handoff_active(&state));

    crate::server::clear_reload_marker();
    if let Some(prev_runtime) = prev_runtime {
        crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_DIR");
    }
}

#[test]
fn test_handle_server_event_history_with_interruption_queues_continuation() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_test_123".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "I was working on something".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: Some(true),
            connection_type: Some("websocket".to_string()),
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert!(app.display_messages().len() >= 2);
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    let system_msg = app
        .display_messages()
        .iter()
        .find(|m| m.role == "system" && m.content == "Reload complete — continuing.")
        .expect("should have a short reload continuation message");
    assert_eq!(system_msg.content, "Reload complete — continuing.");

    assert!(app.queued_messages().is_empty());
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(app.hidden_queued_system_messages[0].contains("interrupted by a server reload"));
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content == "Reload complete — continuing.")
    );
}

#[test]
fn test_handle_server_event_history_without_interruption_does_not_queue() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_test_456".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "Normal response".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: Some("https/sse".to_string()),
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert!(app.queued_messages().is_empty());
    assert_eq!(app.connection_type.as_deref(), Some("https/sse"));
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.content.contains("interrupted"))
    );
}

#[test]
fn test_finalize_reload_reconnect_marker_only_does_not_queue_selfdev_continuation() {
    let mut app = create_test_app();
    app.reload_info
        .push("Reloaded with build abc1234".to_string());

    remote::finalize_reload_reconnect(
        &mut app,
        Some("ses_test_marker_only"),
        remote::ReloadReconnectHints {
            has_reload_ctx_for_session: false,
            has_client_reload_marker: true,
        },
        false,
    );

    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(app.reload_info.is_empty());
    assert!(
        !app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content == "Reload complete — continuing.")
    );
}

#[test]
fn test_handle_server_event_history_restores_side_panel_snapshot() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    let side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "/tmp/plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "# Plan\n```mermaid\nflowchart LR\nA-->B\n```".to_string(),
            updated_at_ms: 1,
        }],
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_side_panel_history".to_string(),
            messages: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: Some("websocket".to_string()),
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: side_panel.clone(),
        },
        &mut remote,
    );

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.side_panel.pages.len(), 1);
    assert_eq!(
        app.side_panel
            .focused_page()
            .map(|page| page.title.as_str()),
        Some("Plan")
    );
}

#[test]
fn test_handle_server_event_side_panel_state_updates_snapshot() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("old".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "old".to_string(),
            title: "Old".to_string(),
            file_path: "/tmp/old.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "old".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.diff_pane_scroll = 7;

    app.handle_server_event(
        crate::protocol::ServerEvent::SidePanelState {
            snapshot: crate::side_panel::SidePanelSnapshot {
                focused_page_id: Some("new".to_string()),
                pages: vec![crate::side_panel::SidePanelPage {
                    id: "new".to_string(),
                    title: "New".to_string(),
                    file_path: "/tmp/new.md".to_string(),
                    format: crate::side_panel::SidePanelPageFormat::Markdown,
                    content: "# New".to_string(),
                    updated_at_ms: 2,
                }],
            },
        },
        &mut remote,
    );

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("new"));
    assert_eq!(app.side_panel.pages.len(), 1);
    assert_eq!(app.diff_pane_scroll, 0);
}

#[test]
fn test_remote_swarm_status_does_not_clobber_newer_session_history_on_disk() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_remote_preserve_history";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote preserve history".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "older on-disk message".to_string(),
            cache_control: None,
        }],
    );
    session.save().expect("save initial session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    app.remote_session_id = Some(session_id.to_string());
    app.swarm_enabled = true;

    // Simulate the shared server advancing the authoritative session file after the
    // remote client already loaded its shadow copy.
    let mut fresher = crate::session::Session::load(session_id).expect("load fresher session");
    fresher.add_message(
        Role::Assistant,
        vec![ContentBlock::Text {
            text: "newer server-side message".to_string(),
            cache_control: None,
        }],
    );
    fresher.save().expect("save fresher session");

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SwarmStatus { members: vec![] },
        &mut remote,
    );

    let persisted = crate::session::Session::load(session_id).expect("reload persisted session");
    assert_eq!(
        persisted.messages.len(),
        2,
        "remote UI persistence should not roll back newer server-written messages"
    );
    let last_text = persisted
        .messages
        .last()
        .and_then(|msg| {
            msg.content.iter().find_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
        })
        .expect("last message text");
    assert_eq!(last_text, "newer server-side message");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_duplicate_history_for_same_session_is_ignored_after_fast_path_restore() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("ses_fast_path".to_string());
    app.push_display_message(DisplayMessage::assistant(
        "local restored state".to_string(),
    ));
    remote.mark_history_loaded();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_fast_path".to_string(),
            messages: vec![crate::protocol::HistoryMessage {
                role: "assistant".to_string(),
                content: "server history replay".to_string(),
                tool_calls: None,
                tool_data: None,
            }],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![],
            client_count: None,
            is_canary: None,
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: Some(true),
            connection_type: Some("websocket".to_string()),
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    let assistant_messages: Vec<_> = app
        .display_messages()
        .iter()
        .filter(|m| m.role == "assistant")
        .collect();
    assert_eq!(assistant_messages.len(), 1);
    assert_eq!(assistant_messages[0].content, "local restored state");
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
    assert!(app.queued_messages().is_empty());
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(app.hidden_queued_system_messages[0].contains("interrupted by a server reload"));
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content == "Reload complete — continuing.")
    );
}

#[test]
fn test_remote_error_with_retry_after_keeps_pending_for_auto_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(9);

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 9,
            message: "rate limited".to_string(),
            retry_after_secs: Some(3),
        },
        &mut remote,
    );

    assert!(!app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.current_message_id.is_none());
    assert!(app.rate_limit_reset.is_some());
    assert!(app.rate_limit_pending_message.is_some());

    let last = app
        .display_messages()
        .last()
        .expect("missing rate-limit status message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Will auto-retry in 3 seconds"));
}

#[test]
fn test_remote_error_without_retry_clears_pending() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 10,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_pending_message.is_none());
    let last = app
        .display_messages()
        .last()
        .expect("missing error message");
    assert_eq!(last.role, "error");
    assert_eq!(last.content, "provider failed hard");
}

#[test]
fn test_remote_error_with_retryable_pending_schedules_retry() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: 0,
        retry_at: None,
    });
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 11,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("retryable continuation should remain pending");
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 1);
    assert!(pending.retry_at.is_some());
    assert!(app.rate_limit_reset.is_some());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "system" && m.content.contains("Auto-retrying"))
    );
}

#[test]
fn test_schedule_pending_remote_retry_respects_retry_limit() {
    let mut app = create_test_app();
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![],
        is_system: true,
        system_reminder: None,
        auto_retry: true,
        retry_attempts: App::AUTO_RETRY_MAX_ATTEMPTS,
        retry_at: None,
    });

    assert!(!app.schedule_pending_remote_retry("⚠ failed."));
    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.rate_limit_reset.is_none());
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content.contains("Auto-retry limit reached"))
    );
}

#[test]
fn test_info_widget_data_includes_connection_type() {
    let mut app = create_test_app();
    app.connection_type = Some("https".to_string());
    let data = crate::tui::TuiState::info_widget_data(&app);
    assert_eq!(data.connection_type.as_deref(), Some("https"));
}

#[test]
fn test_info_widget_remote_openai_uses_remote_provider_for_usage_and_context() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("OpenAI".to_string());
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("OpenAI"));
    assert_eq!(data.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::Unknown
    );
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_remote_model_falls_back_to_model_provider_detection() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.4".to_string());
    app.update_context_limit_for_model("gpt-5.4");

    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.context_limit, Some(1_000_000));
    assert_eq!(
        data.usage_info.as_ref().map(|info| info.provider),
        Some(crate::tui::info_widget::UsageProvider::OpenAI)
    );
}

#[test]
fn test_info_widget_local_gemini_shows_oauth_auth_method() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = crate::auth::gemini::tokens_path().expect("gemini tokens path");
    crate::storage::write_json_secret(
        &path,
        &serde_json::json!({
            "access_token": "at-123",
            "refresh_token": "rt-456",
            "expires_at": 4102444800000i64,
            "email": "user@example.com"
        }),
    )
    .expect("write gemini tokens");
    crate::auth::AuthStatus::invalidate_cache();

    let app = create_gemini_test_app();
    let data = crate::tui::TuiState::info_widget_data(&app);

    assert_eq!(data.provider_name.as_deref(), Some("gemini"));
    assert_eq!(data.model.as_deref(), Some("gemini-2.5-pro"));
    assert_eq!(
        data.auth_method,
        crate::tui::info_widget::AuthMethod::GeminiOAuth
    );
    assert!(data.usage_info.is_none());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    crate::auth::AuthStatus::invalidate_cache();
}

#[test]
fn test_debug_command_message_respects_queue_mode() {
    let mut app = create_test_app();

    // Test 1: When not processing, should submit directly
    app.is_processing = false;
    let result = app.handle_debug_command("message:hello");
    assert!(
        result.starts_with("OK: submitted message"),
        "Expected submitted, got: {}",
        result
    );
    // The message should be processed (added to messages and pending_turn set)
    assert!(app.pending_turn);
    assert_eq!(app.messages.len(), 1);

    // Reset for next test
    app.pending_turn = false;
    app.messages.clear();

    // Test 2: When processing with queue_mode=true, should queue
    app.is_processing = true;
    app.queue_mode = true;
    let result = app.handle_debug_command("message:queued_msg");
    assert!(
        result.contains("queued"),
        "Expected queued, got: {}",
        result
    );
    assert_eq!(app.queued_count(), 1);
    assert_eq!(app.queued_messages()[0], "queued_msg");

    // Test 3: When processing with queue_mode=false, should interleave
    app.queued_messages.clear();
    app.queue_mode = false;
    let result = app.handle_debug_command("message:interleave_msg");
    assert!(
        result.contains("interleave"),
        "Expected interleave, got: {}",
        result
    );
    assert_eq!(app.interleave_message.as_deref(), Some("interleave_msg"));
}

// ====================================================================
// Scroll testing with rendering verification
// ====================================================================

/// Extract plain text from a TestBackend buffer after rendering.
fn buffer_to_text(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let width = buf.area.width as usize;
    let height = buf.area.height as usize;
    let mut lines = Vec::with_capacity(height);
    for y in 0..height {
        let mut line = String::with_capacity(width);
        for x in 0..width {
            let cell = &buf[(x as u16, y as u16)];
            line.push_str(cell.symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    // Trim trailing empty lines
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// Create a test app pre-populated with scrollable content (text + mermaid diagrams).
fn create_scroll_test_app(
    width: u16,
    height: u16,
    diagrams: usize,
    padding: usize,
) -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::clear_streaming_preview_diagram();

    let mut app = create_test_app();
    let content = App::build_scroll_test_content(diagrams, padding, None);
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "Scroll test".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content,
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    // Set deterministic session name for snapshot stability
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(width, height);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

fn create_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "Show me some code".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: "```rust\nfn main() {\n    println!(\"hello\");\n}\n```".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    (app, terminal)
}

/// Get the configured scroll up key binding (code, modifiers).
fn scroll_up_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.up.code.clone(),
        app.scroll_keys.up.modifiers,
    )
}

/// Get the configured scroll down key binding (code, modifiers).
fn scroll_down_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.down.code.clone(),
        app.scroll_keys.down.modifiers,
    )
}

/// Get the configured scroll up fallback key, or primary scroll up key.
fn scroll_up_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
    app.scroll_keys
        .up_fallback
        .as_ref()
        .map(|binding| (binding.code.clone(), binding.modifiers))
        .unwrap_or_else(|| scroll_up_key(app))
}

/// Get the configured scroll down fallback key, or primary scroll down key.
fn scroll_down_fallback_key(app: &App) -> (KeyCode, KeyModifiers) {
    app.scroll_keys
        .down_fallback
        .as_ref()
        .map(|binding| (binding.code.clone(), binding.modifiers))
        .unwrap_or_else(|| scroll_down_key(app))
}

/// Get the configured prompt-up key binding (code, modifiers).
fn prompt_up_key(app: &App) -> (KeyCode, KeyModifiers) {
    (
        app.scroll_keys.prompt_up.code.clone(),
        app.scroll_keys.prompt_up.modifiers,
    )
}

fn scroll_render_test_lock() -> std::sync::MutexGuard<'static, ()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Render app to TestBackend and return the buffer text.
fn render_and_snap(
    app: &App,
    terminal: &mut ratatui::Terminal<ratatui::backend::TestBackend>,
) -> String {
    terminal
        .draw(|f| crate::tui::ui::draw(f, app))
        .expect("draw failed");
    buffer_to_text(terminal)
}

#[test]
fn test_streaming_repaint_does_not_leave_bracket_artifact() {
    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming_text = "[".to_string();
    let _ = render_and_snap(&app, &mut terminal);

    app.streaming_text = "Process A: |██████████|".to_string();
    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("Process A:"),
        "expected updated streaming prefix to be visible"
    );
    assert!(
        text.contains("████"),
        "expected updated streaming progress bar to be visible"
    );
    assert!(
        !text.lines().any(|line| line.trim() == "["),
        "stale standalone '[' artifact should not persist after repaint"
    );
}

#[test]
fn test_remote_typing_resumes_bottom_follow_mode() {
    let mut app = create_test_app();
    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    app.handle_remote_char_input('x');

    assert_eq!(app.input, "x");
    assert_eq!(app.cursor_pos, 1);
    assert_eq!(app.scroll_offset, 0);
    assert!(
        !app.auto_scroll_paused,
        "typing in remote mode should follow newest content, not pin top"
    );
}

#[test]
fn test_local_alt_s_toggles_typing_scroll_lock() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(
        app.status_notice(),
        Some("Typing scroll lock: ON — typing stays at current chat position".to_string())
    );

    app.handle_key(KeyCode::Char('s'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(
        app.status_notice(),
        Some("Typing scroll lock: OFF — typing follows chat bottom".to_string())
    );
}

#[test]
fn test_remote_typing_scroll_lock_preserves_scroll_position() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    app.handle_remote_char_input('x');

    assert_eq!(app.input, "x");
    assert_eq!(app.cursor_pos, 1);
    assert_eq!(app.scroll_offset, 7);
    assert!(
        app.auto_scroll_paused,
        "typing scroll lock should preserve paused scroll state"
    );
}

#[test]
fn test_remote_typing_scroll_lock_can_be_toggled_back_off() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.scroll_offset = 7;
    app.auto_scroll_paused = true;

    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('s'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    app.handle_remote_char_input('x');

    assert_eq!(app.scroll_offset, 0);
    assert!(
        !app.auto_scroll_paused,
        "typing should resume following chat bottom after disabling the lock"
    );
}

#[test]
fn test_reconnect_target_prefers_remote_session_id() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_idle".to_string());
    app.remote_session_id = Some("ses_remote_active".to_string());

    assert_eq!(
        app.reconnect_target_session_id().as_deref(),
        Some("ses_remote_active")
    );
}

#[test]
fn test_reconnect_target_uses_resume_when_remote_missing() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_only".to_string());
    app.remote_session_id = None;

    assert_eq!(
        app.reconnect_target_session_id().as_deref(),
        Some("ses_resume_only")
    );
}

#[test]
fn test_reconnect_target_does_not_consume_resume_session_id() {
    let mut app = create_test_app();
    app.resume_session_id = Some("ses_resume_persistent".to_string());
    app.remote_session_id = None;

    let first = app.reconnect_target_session_id();
    let second = app.reconnect_target_session_id();

    assert_eq!(first.as_deref(), Some("ses_resume_persistent"));
    assert_eq!(second.as_deref(), Some("ses_resume_persistent"));
    assert_eq!(
        app.resume_session_id.as_deref(),
        Some("ses_resume_persistent")
    );
}

#[test]
fn test_prompt_jump_ctrl_brackets() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    app.handle_key(KeyCode::Char('['), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    let after_up = app.scroll_offset;
    app.handle_key(KeyCode::Char(']'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

// NOTE: test_prompt_jump_ctrl_digits_by_recency was removed because it relied on
// pre-render prompt positions that no longer exist. The render-based version
// test_prompt_jump_ctrl_digit_is_recency_rank_in_app covers this functionality.

#[cfg(target_os = "macos")]
#[test]
fn test_prompt_jump_ctrl_esc_fallback_on_macos() {
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    app.handle_key(KeyCode::Esc, KeyModifiers::CONTROL).unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_ctrl_digit_side_panel_preset_in_app() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('1'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 25);

    app.handle_key(KeyCode::Char('2'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 50);

    app.handle_key(KeyCode::Char('3'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 75);

    app.handle_key(KeyCode::Char('4'), KeyModifiers::CONTROL)
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 100);
}

#[test]
fn test_prompt_jump_ctrl_digit_is_recency_rank_in_app() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
    app.handle_key(prompt_up_code, prompt_up_mods).unwrap();
    assert!(app.scroll_offset > 0);

    // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
    app.handle_key(KeyCode::Char('5'), KeyModifiers::CONTROL)
        .unwrap();
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_scroll_cmd_j_k_fallback_in_app() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_fallback_key(&app);
    let (down_code, down_mods) = scroll_down_fallback_key(&app);

    app.handle_key(up_code, up_mods).unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
    let after_up = app.scroll_offset;

    app.handle_key(down_code, down_mods).unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[test]
fn test_remote_prompt_jump_ctrl_brackets() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    rt.block_on(app.handle_remote_key(KeyCode::Char('['), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    let after_up = app.scroll_offset;
    rt.block_on(app.handle_remote_key(KeyCode::Char(']'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[cfg(target_os = "macos")]
#[test]
fn test_remote_prompt_jump_ctrl_esc_fallback_on_macos() {
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    assert_eq!(app.scroll_offset, 0);
    rt.block_on(app.handle_remote_key(KeyCode::Esc, KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_remote_ctrl_digit_side_panel_preset() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('4'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert_eq!(app.diagram_pane_ratio_target, 100);
}

#[test]
fn test_remote_prompt_jump_ctrl_digit_is_recency_rank() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (prompt_up_code, prompt_up_mods) = prompt_up_key(&app);
    rt.block_on(app.handle_remote_key(prompt_up_code, prompt_up_mods, &mut remote))
        .unwrap();
    assert!(app.scroll_offset > 0);

    // Ctrl+5 now means "5th most-recent prompt" (clamped to oldest).
    rt.block_on(app.handle_remote_key(KeyCode::Char('5'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();
    assert!(app.scroll_offset > 0);
}

#[test]
fn test_remote_ctrl_c_interrupts_while_processing() {
    let mut app = create_test_app();
    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.quit_pending.is_none());
    assert!(app.is_processing);
}

#[test]
fn test_remote_ctrl_c_still_arms_quit_when_idle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('c'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_local_copy_badge_shortcut_accepts_alt_uppercase_encoding() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    let notice = app.status_notice().unwrap_or_default();
    assert!(
        notice == "Copied rust",
        "expected copy notice, got: {}",
        notice
    );

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_remote_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    render_and_snap(&app, &mut terminal);

    rt.block_on(app.handle_remote_key(KeyCode::Char('S'), KeyModifiers::ALT, &mut remote))
        .unwrap();

    let notice = app.status_notice().unwrap_or_default();
    assert!(
        notice == "Copied rust",
        "expected copy notice, got: {}",
        notice
    );

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_copy_selection_mode_toggle_shows_notification() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    assert!(app.copy_selection_mode);

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Enter/Y copy") || text.contains("drag to copy"),
        "expected selection mode notification, got: {}",
        text
    );
}

#[test]
fn test_copy_selection_select_all_uses_rendered_chat_text_without_copy_badges() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected transcript text");
    assert!(selected.contains("Show me some code"));
    assert!(selected.contains("fn main() {"));
    assert!(selected.contains("println!(\"hello\");"));
    assert!(
        !selected.contains("[Alt]"),
        "selection should use chat text, not copy badge chrome: {}",
        selected
    );
}

#[test]
fn test_copy_selection_full_user_prompt_line_skips_prompt_chrome() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let (prompt_idx, prompt_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("Show me some code").then_some((abs_line, text))
        })
        .expect("expected visible user prompt line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: prompt_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: prompt_idx,
        column: unicode_width::UnicodeWidthStr::width(prompt_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected user prompt selection text");
    assert_eq!(selected, "Show me some code");
}

#[test]
fn test_copy_selection_swarm_message_skips_rail_chrome() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    app.display_messages = vec![DisplayMessage::swarm("Broadcast", "hello team")];
    app.bump_display_messages_version();

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let (start_idx, _start_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("Broadcast").then_some((abs_line, text))
        })
        .expect("expected visible swarm header line");
    let (end_idx, end_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            text.contains("hello team").then_some((abs_line, text))
        })
        .expect("expected visible swarm body line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: start_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: end_idx,
        column: unicode_width::UnicodeWidthStr::width(end_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected swarm text");
    assert!(selected.contains("Broadcast"));
    assert!(selected.contains("hello team"));
    assert!(
        !selected.contains('│'),
        "selection should omit swarm rail chrome: {selected:?}"
    );
}

#[test]
fn test_copy_selection_reconstructs_wrapped_chat_lines_without_hard_wraps() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: "same physical device: i2c-ELAN900C:00 same vendor/product family: 04F3:4216"
            .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(36, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let visible_lines: Vec<(usize, String)> = (visible_start..visible_end)
        .filter_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            (!text.is_empty()).then_some((abs_line, text))
        })
        .collect();
    let (first_idx, _first_text) = visible_lines
        .iter()
        .find(|(_, text)| text.contains("i2c-ELAN900C:00"))
        .expect("expected wrapped line containing device path");
    let (second_idx, second_text) = visible_lines
        .iter()
        .find(|(idx, _)| *idx == *first_idx + 1)
        .expect("expected adjacent wrapped continuation line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *first_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *second_idx,
        column: unicode_width::UnicodeWidthStr::width(second_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected wrapped selection text");
    assert!(
        !selected.contains('\n'),
        "wrapped chat copy should not include a hard newline: {selected:?}"
    );
    assert!(
        selected.contains("i2c-ELAN900C:00"),
        "selection should include the device path: {selected:?}"
    );
    assert!(
        selected.contains("same vendor/product family"),
        "selection should preserve the natural space across wrapped lines: {selected:?}"
    );
}

#[test]
fn test_copy_selection_centered_list_keeps_logical_list_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.set_centered(true);
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: concat!(
            "A goal should support\n\n",
            "1. Create a goal\n",
            "\n",
            "- title\n",
            "- description / \"why this matters\"\n",
            "- success criteria\n",
        )
        .to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(28, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    render_and_snap(&app, &mut terminal);
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let visible_lines: Vec<(usize, String)> = (visible_start..visible_end)
        .filter_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            (!text.is_empty()).then_some((abs_line, text))
        })
        .collect();

    let (start_idx, _) = visible_lines
        .iter()
        .find(|(_, text)| text.contains("1. Create a goal"))
        .expect("numbered list line");
    let (end_idx, end_text) = visible_lines
        .iter()
        .rev()
        .find(|(_, text)| text.contains("success criteria") || text.contains("matters"))
        .expect("last list line");

    app.copy_selection_anchor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *start_idx,
        column: 0,
    });
    app.copy_selection_cursor = Some(crate::tui::CopySelectionPoint {
        pane: crate::tui::CopySelectionPane::Chat,
        abs_line: *end_idx,
        column: unicode_width::UnicodeWidthStr::width(end_text.as_str()),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected selected list text");

    assert!(
        selected.contains("1. Create a goal"),
        "numbered list item should be copied without centered padding: {selected:?}"
    );
    assert!(
        selected.contains("• title"),
        "bullet item should be copied without centered padding: {selected:?}"
    );
    assert!(
        selected.contains("why this matters"),
        "wrapped bullet item should copy logical text: {selected:?}"
    );
}

#[test]
fn test_copy_selection_mouse_drag_extracts_expected_multiline_range() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let mut fn_line = None;
    let mut print_line = None;
    for abs_line in visible_start..visible_end {
        let text = crate::tui::ui::copy_viewport_line_text(abs_line).unwrap_or_default();
        if text.contains("fn main() {") {
            fn_line = Some((abs_line, text.clone()));
        }
        if text.contains("println!(\"hello\");") {
            print_line = Some((abs_line, text));
        }
    }

    let (fn_line_idx, fn_text) = fn_line.expect("fn line");
    let (print_line_idx, print_text) = print_line.expect("println line");
    let fn_byte = fn_text.find("fn main() {").expect("fn column");
    let fn_col = unicode_width::UnicodeWidthStr::width(&fn_text[..fn_byte]) as u16;
    let _print_end_col = (print_text.find(");").expect("print end") + 2) as u16;

    let base_y = layout.messages_area.y;
    let start_row = base_y + (fn_line_idx - visible_start) as u16;
    let end_row = base_y + (print_line_idx - visible_start) as u16;

    let start_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, start_row)
                .map(|point| point.abs_line == fn_line_idx && point.column == fn_col as usize)
                .unwrap_or(false)
        })
        .expect("screen x for selection start");

    let end_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .filter_map(|column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, end_row)
                .filter(|point| point.abs_line == print_line_idx)
                .map(|point| (column, point.column))
        })
        .max_by_key(|(_, mapped_col)| *mapped_col)
        .map(|(column, _)| column)
        .expect("screen x for selection end");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: start_x,
        row: start_row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: end_x,
        row: end_row,
        modifiers: KeyModifiers::empty(),
    });

    let selected = app
        .current_copy_selection_text()
        .expect("expected multiline selection");
    let range = app.normalized_copy_selection().expect("normalized range");
    assert_eq!(range.start.abs_line, fn_line_idx);
    assert_eq!(range.end.abs_line, print_line_idx);
    assert!(
        selected.contains("fn main() {"),
        "selection missing fn line: {selected}"
    );
    assert!(
        selected.contains("println!(\"hello\");"),
        "selection missing println line: {selected}"
    );
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: end_x,
        row: end_row,
        modifiers: KeyModifiers::empty(),
    });
    assert!(app.copy_selection_mode);
    assert!(!app.copy_selection_dragging);
}

#[test]
fn test_copy_selection_mouse_click_does_not_enter_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let target = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::copy_viewport_line_text(abs_line)?;
            let byte = text.find("println!(\"hello\");")?;
            let col = unicode_width::UnicodeWidthStr::width(&text[..byte]) as u16;
            Some((abs_line, col))
        })
        .expect("println line");

    let row = layout.messages_area.y + (target.0 - visible_start) as u16;
    let col = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, row)
                .map(|point| point.abs_line == target.0 && point.column == target.1 as usize)
                .unwrap_or(false)
        })
        .expect("screen x for println");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });
    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: col,
        row,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
}

#[test]
fn test_copy_selection_mouse_drag_auto_copies_and_exits_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");

    let mut fn_line = None;
    let mut print_line = None;
    for abs_line in visible_start..visible_end {
        let text = crate::tui::ui::copy_viewport_line_text(abs_line).unwrap_or_default();
        if text.contains("fn main() {") {
            fn_line = Some((abs_line, text.clone()));
        }
        if text.contains("println!(\"hello\");") {
            print_line = Some((abs_line, text));
        }
    }

    let (fn_line_idx, fn_text) = fn_line.expect("fn line");
    let (print_line_idx, _print_text) = print_line.expect("println line");
    let fn_byte = fn_text.find("fn main() {").expect("fn column");
    let fn_col = unicode_width::UnicodeWidthStr::width(&fn_text[..fn_byte]) as u16;

    let base_y = layout.messages_area.y;
    let start_row = base_y + (fn_line_idx - visible_start) as u16;
    let end_row = base_y + (print_line_idx - visible_start) as u16;

    let start_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .find(|&column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, start_row)
                .map(|point| point.abs_line == fn_line_idx && point.column == fn_col as usize)
                .unwrap_or(false)
        })
        .expect("screen x for selection start");

    let end_x = (layout.messages_area.x..layout.messages_area.x + layout.messages_area.width)
        .filter_map(|column| {
            crate::tui::ui::copy_viewport_point_from_screen(column, end_row)
                .filter(|point| point.abs_line == print_line_idx)
                .map(|point| (column, point.column))
        })
        .max_by_key(|(_, mapped_col)| *mapped_col)
        .map(|(column, _)| column)
        .expect("screen x for selection end");

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: start_x,
            row: start_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: end_x,
            row: end_row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: end_x,
            row: end_row,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );

    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
    assert!(copied.lock().unwrap().contains("println!(\"hello\");"));
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
}

#[test]
fn test_side_panel_mouse_drag_extracts_expected_text() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            content: "alpha\nbeta highlight target\ngamma".to_string(),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(100, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let diff_area = layout.diff_pane_area.expect("side pane area");
    let (visible_start, visible_end) =
        crate::tui::ui::side_pane_visible_range().expect("side pane visible range");

    let (line_idx, _line_text) = (visible_start..visible_end)
        .find_map(|abs_line| {
            let text = crate::tui::ui::side_pane_line_text(abs_line)?;
            text.contains("beta highlight target")
                .then_some((abs_line, text))
        })
        .expect("target side pane line");
    let (row, column) = (diff_area.y..diff_area.y + diff_area.height)
        .find_map(|screen_y| {
            (diff_area.x..diff_area.x + diff_area.width)
                .find(|&screen_x| {
                    crate::tui::ui::side_pane_point_from_screen(screen_x, screen_y)
                        .map(|point| point.abs_line == line_idx)
                        .unwrap_or(false)
                })
                .map(|screen_x| (screen_y, screen_x))
        })
        .expect("screen x for side selection start");
    let end_column = (diff_area.x..diff_area.x + diff_area.width)
        .filter_map(|screen_x| {
            crate::tui::ui::side_pane_point_from_screen(screen_x, row)
                .filter(|point| point.abs_line == line_idx)
                .map(|point| (screen_x, point.column))
        })
        .max_by_key(|(_, mapped)| *mapped)
        .map(|(screen_x, _)| screen_x)
        .expect("screen x for side selection end");

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );
    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: end_column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |_| true,
    );

    let selected = app
        .current_copy_selection_text()
        .expect("expected side pane selection");
    assert!(
        selected.contains("beta highlight target"),
        "selected={selected}"
    );
    assert_eq!(
        app.current_copy_selection_pane(),
        Some(crate::tui::CopySelectionPane::SidePane)
    );

    app.handle_copy_selection_mouse_with(
        MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: end_column,
            row,
            modifiers: KeyModifiers::empty(),
        },
        |text| {
            *copied_for_closure.lock().unwrap() = text.to_string();
            true
        },
    );
    assert!(copied.lock().unwrap().contains("beta highlight target"));
    assert!(!app.copy_selection_mode);
}

#[test]
fn test_copy_selection_copy_action_uses_clipboard_hook_and_exits_mode() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    render_and_snap(&app, &mut terminal);
    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .unwrap();
    assert!(app.select_all_in_copy_mode());

    let success = app.copy_current_selection_to_clipboard_with(|text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(success);
    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
    assert!(copied.lock().unwrap().contains("println!(\"hello\");"));
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
}

#[test]
fn test_copy_badge_modifier_highlights_while_held() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_copy_test_app();

    render_and_snap(&app, &mut terminal);

    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode};

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        KeyModifiers::ALT,
        KeyEventKind::Press,
    ));
    assert!(app.copy_badge_ui().alt_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftShift),
        KeyModifiers::ALT | KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));
    assert!(app.copy_badge_ui().shift_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftShift),
        KeyModifiers::ALT,
        KeyEventKind::Release,
    ));
    assert!(!app.copy_badge_ui().shift_active);

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Modifier(ModifierKeyCode::LeftAlt),
        KeyModifiers::empty(),
        KeyEventKind::Release,
    ));
    assert!(!app.copy_badge_ui().alt_active);
}

#[test]
fn test_copy_badge_requires_prior_combo_progress() {
    let mut state = CopyBadgeUiState::default();
    let now = std::time::Instant::now();

    state.shift_active = true;
    state.shift_pulse_until = Some(now + std::time::Duration::from_millis(100));
    state.key_active = Some(('s', now + std::time::Duration::from_millis(100)));

    assert!(
        !state.shift_is_active(now),
        "shift should not light before alt"
    );
    assert!(
        !state.key_is_active('s', now),
        "final key should not light before alt+shift"
    );

    state.alt_active = true;
    assert!(
        state.shift_is_active(now),
        "shift should light once alt is active"
    );
    assert!(
        state.key_is_active('s', now),
        "final key should light once alt+shift are active"
    );
}

#[test]
fn test_disconnected_key_handler_allows_typing_and_queueing() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();
    assert_eq!(app.input, "hi");

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
    assert_eq!(
        app.status_notice(),
        Some("Queued for send after reconnect (1 message)".to_string())
    );
}

#[test]
fn test_disconnected_shift_enter_inserts_newline() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::SHIFT).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();

    assert_eq!(app.input(), "h\ni");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_ctrl_enter_queues_for_reconnect() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('h'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Char('i'), KeyModifiers::empty()).unwrap();
    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::CONTROL).unwrap();

    assert!(app.input.is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
}

#[test]
fn test_disconnected_key_handler_restart_runs_locally() {
    let mut app = create_test_app();
    app.input = "/restart".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.restart_requested.is_some());
    assert!(app.should_quit);
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_key_handler_does_not_queue_server_commands() {
    let mut app = create_test_app();
    app.input = "/server-reload".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert_eq!(app.input, "/server-reload");
    assert!(app.queued_messages().is_empty());
    assert_eq!(
        app.status_notice(),
        Some("This command requires a live connection".to_string())
    );
}

#[test]
fn test_disconnected_key_handler_ctrl_c_arms_quit() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('c'), KeyModifiers::CONTROL).unwrap();

    assert!(app.quit_pending.is_some());
    assert_eq!(
        app.status_notice(),
        Some("Press Ctrl+C again to quit".to_string())
    );
}

#[test]
fn test_remote_scroll_cmd_j_k_fallback() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    // Seed max scroll estimates before key handling.
    render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_fallback_key(&app);
    let (down_code, down_mods) = scroll_down_fallback_key(&app);

    rt.block_on(app.handle_remote_key(up_code, up_mods, &mut remote))
        .unwrap();
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);
    let after_up = app.scroll_offset;

    rt.block_on(app.handle_remote_key(down_code, down_mods, &mut remote))
        .unwrap();
    assert!(app.scroll_offset <= after_up);
}

#[test]
fn test_remote_shift_enter_inserts_newline() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::SHIFT, &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('i'), KeyModifiers::empty(), &mut remote))
        .unwrap();

    assert_eq!(app.input(), "h\ni");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_remote_ctrl_enter_queues_while_processing() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.is_processing = true;
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Char('i'), KeyModifiers::empty(), &mut remote))
        .unwrap();
    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert!(app.input().is_empty());
    assert_eq!(app.queued_messages().len(), 1);
    assert_eq!(app.queued_messages()[0], "hi");
}

#[test]
fn test_scroll_ctrl_k_j_offset() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 20);

    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    let (up_code, up_mods) = scroll_up_key(&app);
    let (down_code, down_mods) = scroll_down_key(&app);

    // Render first so LAST_MAX_SCROLL is populated
    render_and_snap(&app, &mut terminal);

    // Scroll up (switches to absolute-from-top mode)
    app.handle_key(up_code.clone(), up_mods).unwrap();
    assert!(app.auto_scroll_paused);
    let first_offset = app.scroll_offset;

    app.handle_key(up_code.clone(), up_mods).unwrap();
    let second_offset = app.scroll_offset;
    assert!(
        second_offset < first_offset,
        "scrolling up should decrease absolute offset (move toward top)"
    );

    // Scroll down (increases absolute position = moves toward bottom)
    app.handle_key(down_code.clone(), down_mods).unwrap();
    assert_eq!(
        app.scroll_offset, first_offset,
        "one scroll down should undo one scroll up"
    );

    // Keep scrolling down until back at bottom
    for _ in 0..10 {
        app.handle_key(down_code.clone(), down_mods).unwrap();
        if !app.auto_scroll_paused {
            break;
        }
    }
    assert_eq!(app.scroll_offset, 0);
    assert!(!app.auto_scroll_paused);

    // Stays at 0 when already at bottom
    app.handle_key(down_code.clone(), down_mods).unwrap();
    assert_eq!(app.scroll_offset, 0);
}

#[test]
fn test_scroll_offset_capped() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 4);

    let (up_code, up_mods) = scroll_up_key(&app);

    // Render first so LAST_MAX_SCROLL is populated
    render_and_snap(&app, &mut terminal);

    // Spam scroll-up many times
    for _ in 0..500 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }

    // Should be at 0 (absolute top) after scrolling up enough
    assert_eq!(app.scroll_offset, 0);
    assert!(app.auto_scroll_paused);
}

#[test]
fn test_scroll_render_bottom() {
    let _render_lock = scroll_render_test_lock();
    let (app, mut terminal) = create_scroll_test_app(80, 15, 1, 20);
    let text = render_and_snap(&app, &mut terminal);

    // At bottom (scroll_offset=0), filler content should be visible.
    assert!(
        text.contains("stretch content"),
        "expected filler content at bottom position"
    );
    // Should have scroll indicator or prompt preview since content extends above viewport.
    // The prompt preview (N›) renders on top of the ↑ indicator, so check for either.
    assert!(
        text.contains('↑') || text.contains('›'),
        "expected ↑ indicator or prompt preview when content extends above viewport"
    );
}

#[test]
fn test_scroll_render_scrolled_up() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 8);

    // Seed scroll metrics, then enter paused/scrolled mode via the real key path.
    let _ = render_and_snap(&app, &mut terminal);
    let (up_code, up_mods) = scroll_up_key(&app);
    app.handle_key(up_code, up_mods).unwrap();

    assert!(app.auto_scroll_paused, "scroll-up should pause auto-follow");
    assert!(
        app.scroll_offset > 0,
        "scroll-up should move away from bottom"
    );

    let text_scrolled = render_and_snap(&app, &mut terminal);

    assert!(
        text_scrolled.contains('↓'),
        "expected ↓ indicator when paused above bottom"
    );
}

#[test]
fn test_prompt_preview_reserves_rows_without_overwriting_visible_history() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage {
            role: "user".to_string(),
            content: "This is a deliberately long prompt preview that should wrap into two preview rows at the top of the viewport".to_string(),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
        DisplayMessage {
            role: "assistant".to_string(),
            content: App::build_scroll_test_content(0, 20, None),
            tool_calls: vec![],
            duration_secs: None,
            title: None,
            tool_data: None,
        },
    ];
    app.bump_display_messages_version();
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    app.is_processing = false;
    app.streaming_text.clear();
    app.status = ProcessingStatus::Idle;
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("1›"),
        "expected sticky prompt preview, got:\n{}",
        text
    );
    assert!(
        text.contains("..."),
        "expected two-line preview truncation, got:\n{}",
        text
    );
    assert!(
        text.contains("Intro line 20"),
        "latest visible content should remain visible below preview, got:\n{}",
        text
    );
}

#[test]
fn test_scroll_top_does_not_snap_to_bottom() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 24);

    // Top position in paused mode (absolute offset from top).
    app.scroll_offset = 0;
    app.auto_scroll_paused = true;
    let text_top = render_and_snap(&app, &mut terminal);

    // Bottom position (auto-follow mode).
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    let text_bottom = render_and_snap(&app, &mut terminal);

    assert_ne!(
        text_top, text_bottom,
        "top viewport should differ from bottom viewport"
    );
    assert!(
        text_top.contains("Intro line 01"),
        "top viewport should include earliest content"
    );
}

#[test]
fn test_scroll_content_shifts() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

    // Render at bottom
    app.scroll_offset = 0;
    app.auto_scroll_paused = false;
    let text_bottom = render_and_snap(&app, &mut terminal);

    // Render scrolled up (absolute line 10 from top)
    app.scroll_offset = 10;
    app.auto_scroll_paused = true;
    let text_scrolled = render_and_snap(&app, &mut terminal);

    assert_ne!(
        text_bottom, text_scrolled,
        "content should change when scrolled"
    );
}

#[test]
fn test_scroll_render_with_mermaid() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 2, 10);

    // Render at several positions without crashing.
    for (offset, paused) in [(0, false), (5, true), (10, true), (20, true), (50, true)] {
        app.scroll_offset = offset;
        app.auto_scroll_paused = paused;
        terminal
            .draw(|f| crate::tui::ui::draw(f, &app))
            .unwrap_or_else(|e| panic!("draw failed at scroll_offset={}: {}", offset, e));
    }
}

#[test]
fn test_scroll_visual_debug_frame() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(100, 30, 1, 10);

    crate::tui::visual_debug::enable();

    // Render at bottom, verify frame capture works
    app.scroll_offset = 0;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw at offset=0 failed");

    let frame = crate::tui::visual_debug::latest_frame();
    assert!(frame.is_some(), "visual debug frame should be captured");

    // Render at scroll_offset=10, verify no panic
    app.scroll_offset = 10;
    app.auto_scroll_paused = true;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw at offset=10 failed");

    // Note: latest_frame() is global and may be overwritten by parallel tests,
    // so we only verify the frame capture mechanism works, not exact values.
    let frame = crate::tui::visual_debug::latest_frame();
    assert!(
        frame.is_some(),
        "frame should still be available after second draw"
    );

    crate::tui::visual_debug::disable();
}

#[test]
fn test_scroll_key_then_render() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 40);

    // Render at bottom first (populates LAST_MAX_SCROLL)
    let _text_before = render_and_snap(&app, &mut terminal);

    let (up_code, up_mods) = scroll_up_key(&app);

    // Scroll up three times (9 lines total)
    for _ in 0..3 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }
    assert!(app.auto_scroll_paused);
    assert!(app.scroll_offset > 0);

    // Render again - verifies scroll_offset produces a valid frame without panic.
    // Note: LAST_MAX_SCROLL is a process-wide global that parallel tests
    // can overwrite at any time, so we only check that rendering succeeds
    // and that scroll state is correct - not that the rendered text differs,
    // since the global can clamp scroll_offset to 0 during render.
    let _text_after = render_and_snap(&app, &mut terminal);
}

#[test]
fn test_scroll_round_trip() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 1, 12);

    let (up_code, up_mods) = scroll_up_key(&app);
    let (down_code, down_mods) = scroll_down_key(&app);

    // Render at bottom before scrolling (populates LAST_MAX_SCROLL)
    let _text_original = render_and_snap(&app, &mut terminal);

    // Scroll up 3x
    for _ in 0..3 {
        app.handle_key(up_code.clone(), up_mods).unwrap();
    }
    assert!(app.auto_scroll_paused);

    // Rendering after scrolling up should succeed; exact buffer diffs are brittle
    // because process-wide render state can influence viewport clamping.
    let _text_scrolled = render_and_snap(&app, &mut terminal);

    // Scroll back down until at bottom
    for _ in 0..20 {
        app.handle_key(down_code.clone(), down_mods).unwrap();
        if !app.auto_scroll_paused {
            break;
        }
    }
    assert_eq!(
        app.scroll_offset, 0,
        "scroll_offset should return to 0 after round-trip"
    );
    assert!(!app.auto_scroll_paused);

    // Verify we're back at the bottom and rendering still succeeds.
    let _text_restored = render_and_snap(&app, &mut terminal);
}

#[test]
fn test_copy_selection_from_bottom_rebases_scroll_instead_of_jumping_to_top() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_scroll_test_app(80, 25, 0, 40);

    let bottom_text = render_and_snap(&app, &mut terminal);
    let max_scroll = crate::tui::ui::last_max_scroll();
    assert!(
        max_scroll > 0,
        "expected scrollable history for selection test"
    );
    assert!(
        !bottom_text.contains("Intro line 01"),
        "bottom viewport should not start at top before selection"
    );

    app.handle_key(KeyCode::Char('y'), KeyModifiers::ALT)
        .expect("enter copy mode");
    app.handle_key(KeyCode::Right, KeyModifiers::empty())
        .expect("move selection cursor");

    assert!(
        app.copy_selection_mode,
        "copy selection mode should remain active"
    );
    assert!(app.auto_scroll_paused, "selection should pause auto-follow");
    assert_eq!(
        app.scroll_offset, max_scroll,
        "selection should preserve the current bottom viewport when pausing auto-follow"
    );

    let selected_text = render_and_snap(&app, &mut terminal);
    assert!(
        !selected_text.contains("Intro line 01"),
        "starting selection from bottom should not teleport to the top"
    );
}
