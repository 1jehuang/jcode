#![cfg_attr(test, allow(clippy::clone_on_copy))]

use super::*;
use crate::bus::{
    BackgroundTaskCompleted, BackgroundTaskStatus, BusEvent, ClientMaintenanceAction,
    InputShellCompleted, SessionUpdateStatus,
};
use crate::tui::TuiState;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::cell::RefCell;
use std::sync::{Arc as StdArc, Mutex as StdMutex};
use std::time::{Duration, Instant};

fn cleanup_background_task_files(task_id: &str) {
    let task_dir = std::env::temp_dir().join("jcode-bg-tasks");
    let _ = std::fs::remove_file(task_dir.join(format!("{}.status.json", task_id)));
    let _ = std::fs::remove_file(task_dir.join(format!("{}.output", task_id)));
}

pub(super) fn cleanup_reload_context_file(session_id: &str) {
    if let Ok(path) = crate::tool::selfdev::ReloadContext::path_for_session(session_id) {
        let _ = std::fs::remove_file(path);
    }
}

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
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[test]
fn local_add_provider_message_does_not_retain_local_provider_copy() {
    let mut app = create_test_app();
    app.add_provider_message(Message::user("hello"));
    assert!(app.messages.is_empty());
}

#[test]
fn remote_add_provider_message_retains_remote_provider_copy() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.add_provider_message(Message::user("hello"));
    assert_eq!(app.messages.len(), 1);
}

#[test]
fn debug_memory_profile_includes_app_owned_summary_for_large_client_state() {
    let mut app = create_test_app();
    app.remote_side_pane_images
        .push(crate::session::RenderedImage {
            media_type: "image/png".to_string(),
            data: "x".repeat(32 * 1024),
            label: Some("preview.png".to_string()),
            source: crate::session::RenderedImageSource::UserInput,
        });
    app.observe_page_markdown = "# observe\n".repeat(256);
    app.input_undo_stack.push(("draft ".repeat(256), 12));

    let profile = app.debug_memory_profile();
    let app_owned = &profile["app_owned"];
    let summary = &profile["summary"];

    assert!(app_owned.is_object());
    assert!(summary.is_object());
    assert!(
        app_owned["images_and_views"]["remote_side_pane_images_bytes"]
            .as_u64()
            .unwrap_or(0)
            >= 32 * 1024
    );
    assert!(
        app_owned["input_history"]["undo_stack_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(
        summary["total_app_owned_estimate_bytes"]
            .as_u64()
            .unwrap_or(0)
            > 0
    );
    assert!(
        !summary["top_buckets"]
            .as_array()
            .unwrap_or(&Vec::new())
            .is_empty()
    );
}

fn test_side_panel_snapshot(page_id: &str, title: &str) -> crate::side_panel::SidePanelSnapshot {
    crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some(page_id.to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: page_id.to_string(),
            title: title.to_string(),
            file_path: format!("/tmp/{page_id}.md"),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: format!("# {title}"),
            updated_at_ms: 1,
        }],
    }
}

fn ensure_test_jcode_home_if_unset() {
    use std::sync::OnceLock;

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

fn clear_persisted_test_ui_state() {
    if let Ok(home) = crate::storage::jcode_dir() {
        let ambient_dir = home.join("ambient");
        let _ = std::fs::remove_file(ambient_dir.join("queue.json"));
        let _ = std::fs::remove_file(ambient_dir.join("state.json"));
        let _ = std::fs::remove_file(ambient_dir.join("directives.json"));
        let _ = std::fs::remove_file(ambient_dir.join("visible_cycle.json"));
    }
    crate::tui::app::helpers::clear_ambient_info_cache_for_tests();
    crate::auth::AuthStatus::invalidate_cache();
}

fn with_temp_jcode_home<T>(f: impl FnOnce() -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);
    crate::auth::AuthStatus::invalidate_cache();
    clear_persisted_test_ui_state();

    let result = f();

    crate::auth::claude::set_active_account_override(None);
    crate::auth::codex::set_active_account_override(None);
    crate::auth::AuthStatus::invalidate_cache();
    crate::tui::app::helpers::clear_ambient_info_cache_for_tests();
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    result
}

fn create_jcode_repo_fixture() -> tempfile::TempDir {
    let temp = tempfile::TempDir::new().expect("temp repo");
    std::fs::create_dir_all(temp.path().join(".git")).expect("git dir");
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"jcode\"\nversion = \"0.1.0\"\n",
    )
    .expect("cargo toml");
    temp
}

fn create_real_git_repo_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().expect("tempdir");
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .expect("git init");
    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(temp.path())
        .output()
        .expect("git config email");
    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(temp.path())
        .output()
        .expect("git config name");
    std::fs::write(temp.path().join("tracked.txt"), "before\n").expect("write tracked file");
    std::process::Command::new("git")
        .args(["add", "tracked.txt"])
        .current_dir(temp.path())
        .output()
        .expect("git add");
    std::process::Command::new("git")
        .args(["commit", "-m", "init"])
        .current_dir(temp.path())
        .output()
        .expect("git commit");
    temp
}

#[test]
fn test_handle_turn_error_failover_prompt_manual_mode_shows_system_notice() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"manual\"\n");
        let mut app = create_test_app();
        let prompt = crate::provider::ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            reason: "OAuth usage exhausted".to_string(),
            estimated_input_chars: 48_000,
            estimated_input_tokens: 12_000,
        };

        app.handle_turn_error(failover_error_message(&prompt));

        let last = app.display_messages.last().expect("display message");
        assert_eq!(last.role, "system");
        assert!(last.content.contains("did **not** resend your prompt"));
        assert!(last.content.contains("/model"));
        assert!(
            last.content
                .contains("cross_provider_failover = \"manual\"")
        );
        assert!(app.pending_provider_failover.is_none());
    });
}

#[test]
fn test_handle_turn_error_failover_prompt_countdown_can_switch_and_retry() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let (mut app, active_provider) = create_switchable_test_app("claude");
        let prompt = crate::provider::ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            reason: "OAuth usage exhausted".to_string(),
            estimated_input_chars: 32_000,
            estimated_input_tokens: 8_000,
        };

        app.handle_turn_error(failover_error_message(&prompt));
        assert!(app.pending_provider_failover.is_some());

        if let Some(pending) = app.pending_provider_failover.as_mut() {
            pending.deadline = Instant::now() - Duration::from_secs(1);
        }
        app.maybe_progress_provider_failover_countdown();

        assert!(app.pending_provider_failover.is_none());
        assert!(app.pending_turn);
        assert_eq!(active_provider.lock().unwrap().as_str(), "openai");
        assert_eq!(app.session.model.as_deref(), Some("gpt-test"));
        let last = app.display_messages.last().expect("display message");
        assert!(
            last.content
                .contains("cross_provider_failover = \"manual\"")
        );
    });
}

#[test]
fn test_cancel_pending_provider_failover_clears_countdown() {
    with_temp_jcode_home(|| {
        write_test_config("[provider]\ncross_provider_failover = \"countdown\"\n");
        let (mut app, _active_provider) = create_switchable_test_app("claude");
        let prompt = crate::provider::ProviderFailoverPrompt {
            from_provider: "claude".to_string(),
            from_label: "Anthropic".to_string(),
            to_provider: "openai".to_string(),
            to_label: "OpenAI".to_string(),
            reason: "OAuth usage exhausted".to_string(),
            estimated_input_chars: 16_000,
            estimated_input_tokens: 4_000,
        };

        app.handle_turn_error(failover_error_message(&prompt));
        assert!(app.pending_provider_failover.is_some());

        app.cancel_pending_provider_failover("Provider auto-switch canceled");

        assert!(app.pending_provider_failover.is_none());
        let last = app.display_messages.last().expect("display message");
        assert_eq!(last.role, "system");
        assert!(last.content.contains("Canceled provider auto-switch"));
        assert!(
            last.content
                .contains("cross_provider_failover = \"manual\"")
        );
    });
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

#[derive(Clone)]
struct SwitchableMockProvider {
    active_provider: StdArc<StdMutex<String>>,
}

#[async_trait::async_trait]
impl Provider for SwitchableMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("SwitchableMockProvider")
    }

    fn name(&self) -> &str {
        "switchable-mock"
    }

    fn model(&self) -> String {
        match self.active_provider.lock().unwrap().as_str() {
            "openai" => "gpt-test".to_string(),
            _ => "claude-test".to_string(),
        }
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }

    fn switch_active_provider_to(&self, provider: &str) -> Result<()> {
        *self.active_provider.lock().unwrap() = provider.to_string();
        Ok(())
    }
}

fn create_switchable_test_app(initial_provider: &str) -> (App, StdArc<StdMutex<String>>) {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let active_provider = StdArc::new(StdMutex::new(initial_provider.to_string()));
    let provider: Arc<dyn Provider> = Arc::new(SwitchableMockProvider {
        active_provider: active_provider.clone(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    (app, active_provider)
}

#[derive(Clone)]
struct AuthRefreshingMockProvider {
    logged_in: StdArc<StdMutex<bool>>,
}

#[async_trait::async_trait]
impl Provider for AuthRefreshingMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AuthRefreshingMockProvider")
    }

    fn name(&self) -> &str {
        "auth-refresh-mock"
    }

    fn model(&self) -> String {
        if *self.logged_in.lock().unwrap() {
            "claude-opus-4.6".to_string()
        } else {
            "gpt-5.4".to_string()
        }
    }

    fn available_models_display(&self) -> Vec<String> {
        if *self.logged_in.lock().unwrap() {
            vec![
                "claude-opus-4.6".to_string(),
                "grok-code-fast-1".to_string(),
            ]
        } else {
            vec!["gpt-5.4".to_string()]
        }
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        if *self.logged_in.lock().unwrap() {
            vec![
                crate::provider::ModelRoute {
                    model: "claude-opus-4.6".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "grok-code-fast-1".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
            ]
        } else {
            vec![crate::provider::ModelRoute {
                model: "gpt-5.4".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            }]
        }
    }

    fn on_auth_changed(&self) {
        *self.logged_in.lock().unwrap() = true;
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_auth_refresh_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(AuthRefreshingMockProvider {
        logged_in: StdArc::new(StdMutex::new(false)),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[derive(Clone)]
struct AntigravityMockProvider {
    model: StdArc<StdMutex<String>>,
}

#[async_trait::async_trait]
impl Provider for AntigravityMockProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("AntigravityMockProvider")
    }

    fn name(&self) -> &str {
        "Antigravity"
    }

    fn model(&self) -> String {
        self.model.lock().unwrap().clone()
    }

    fn set_model(&self, model: &str) -> Result<()> {
        let resolved = model.strip_prefix("antigravity:").unwrap_or(model).to_string();
        *self.model.lock().unwrap() = resolved;
        Ok(())
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![
            crate::provider::ModelRoute {
                model: "claude-sonnet-4-6".to_string(),
                provider: "Antigravity".to_string(),
                api_method: "cli".to_string(),
                available: true,
                detail: "cached catalog".to_string(),
                cheapness: None,
            },
            crate::provider::ModelRoute {
                model: "gpt-oss-120b-medium".to_string(),
                provider: "Antigravity".to_string(),
                api_method: "cli".to_string(),
                available: true,
                detail: "cached catalog".to_string(),
                cheapness: None,
            },
        ]
    }

    fn available_models_display(&self) -> Vec<String> {
        vec![
            "claude-sonnet-4-6".to_string(),
            "gpt-oss-120b-medium".to_string(),
        ]
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_antigravity_picker_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(AntigravityMockProvider {
        model: StdArc::new(StdMutex::new("default".to_string())),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

#[derive(Clone)]
struct FailingModelSwitchProvider;

#[async_trait::async_trait]
impl Provider for FailingModelSwitchProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("FailingModelSwitchProvider")
    }

    fn name(&self) -> &str {
        "failing-model-switch"
    }

    fn model(&self) -> String {
        "gpt-5.4".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![crate::provider::ModelRoute {
            model: "claude-opus-4.6".to_string(),
            provider: "Copilot".to_string(),
            api_method: "copilot".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        }]
    }

    fn set_model(&self, _model: &str) -> Result<()> {
        anyhow::bail!("credentials expired")
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_failing_model_switch_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(FailingModelSwitchProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn write_test_config(contents: &str) {
    let path = crate::config::Config::path().expect("config path");
    std::fs::create_dir_all(path.parent().expect("config dir")).expect("config dir");
    std::fs::write(path, contents).expect("write config");
}

fn failover_error_message(prompt: &crate::provider::ProviderFailoverPrompt) -> String {
    format!(
        "[jcode-provider-failover]{}\nignored",
        serde_json::to_string(prompt).expect("serialize failover prompt")
    )
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
fn session_picker_resume_action_keeps_overlay_open() {
    let mut app = create_test_app();
    app.session_picker_mode = SessionPickerMode::CatchUp;
    app.session_picker_overlay = Some(RefCell::new(
        crate::tui::session_picker::SessionPicker::new(vec![
            crate::tui::session_picker::SessionInfo {
                id: "session_keep_open".to_string(),
                parent_id: None,
                short_name: "keep-open".to_string(),
                icon: "k".to_string(),
                title: "Keep Open".to_string(),
                message_count: 1,
                user_message_count: 1,
                assistant_message_count: 0,
                created_at: chrono::Utc::now(),
                last_message_time: chrono::Utc::now(),
                last_active_at: None,
                working_dir: None,
                model: None,
                provider_key: None,
                is_canary: false,
                is_debug: false,
                saved: false,
                save_label: None,
                status: crate::session::SessionStatus::Closed,
                needs_catchup: false,
                estimated_tokens: 0,
                messages_preview: Vec::new(),
                search_index: "keep-open keep open".to_string(),
                server_name: None,
                server_icon: None,
                source: crate::tui::session_picker::SessionSource::Jcode,
                resume_target: crate::tui::session_picker::ResumeTarget::JcodeSession {
                    session_id: "session_keep_open".to_string(),
                },
                external_path: None,
            },
        ]),
    ));

    app.handle_session_picker_key(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::empty(),
    )
    .expect("session picker enter should succeed");

    assert!(app.session_picker_overlay.is_some());
}

#[test]
fn test_resize_redraw_is_debounced() {
    let mut app = create_test_app();

    assert!(app.should_redraw_after_resize());
    assert!(!app.should_redraw_after_resize());

    app.last_resize_redraw = Some(Instant::now() - Duration::from_millis(40));
    assert!(app.should_redraw_after_resize());
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
fn test_help_topic_shows_btw_command_details() {
    let mut app = create_test_app();
    app.input = "/help btw".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/btw <question>`"));
    assert!(msg.content.contains("side panel"));
}

#[test]
fn test_help_topic_shows_git_command_details() {
    let mut app = create_test_app();
    app.input = "/help git".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/git`"));
    assert!(msg.content.contains("git status --short --branch"));
    assert!(msg.content.contains("`/git status`"));
}

#[test]
fn test_help_topic_shows_catchup_command_details() {
    let mut app = create_test_app();
    app.input = "/help catchup".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/catchup`"));
    assert!(msg.content.contains("side panel"));
    assert!(msg.content.contains("`/catchup next`"));
}

#[test]
fn test_help_topic_shows_back_command_details() {
    let mut app = create_test_app();
    app.input = "/help back".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/back`"));
    assert!(msg.content.contains("Catch Up"));
}

#[test]
fn test_catchup_next_queues_resume_for_attention_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.remote_session_id = Some(app.session.id.clone());

        let mut target = Session::create(None, Some("catchup target".to_string()));
        target.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "Review the implementation and summarize what changed.".to_string(),
                cache_control: None,
            }],
        );
        target.add_message(
            crate::message::Role::Assistant,
            vec![crate::message::ContentBlock::Text {
                text: "I finished the work and need your decision on the next step.".to_string(),
                cache_control: None,
            }],
        );
        target.mark_closed();
        target.save().expect("save catchup target");

        app.input = "/catchup next".to_string();
        app.submit_input();

        let pending = app
            .pending_catchup_resume
            .clone()
            .expect("missing pending catchup resume");
        assert_eq!(pending.target_session_id, target.id);
        assert_eq!(pending.source_session_id, app.remote_session_id);
        assert_eq!(pending.queue_position, Some((1, 1)));
        assert!(pending.show_brief);

        let msg = app
            .display_messages()
            .last()
            .expect("missing catchup queued message");
        assert_eq!(msg.role, "system");
        assert!(msg.content.contains("Queued Catch Up"));
    });
}

#[test]
fn test_back_command_queues_return_without_showing_brief() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.catchup_return_stack.push("session_prev".to_string());

    app.input = "/back".to_string();
    app.submit_input();

    let pending = app
        .pending_catchup_resume
        .clone()
        .expect("missing pending back resume");
    assert_eq!(pending.target_session_id, "session_prev");
    assert_eq!(pending.source_session_id, None);
    assert_eq!(pending.queue_position, None);
    assert!(!pending.show_brief);
}

#[test]
fn test_maybe_show_catchup_after_history_adds_brief_page_and_marks_seen() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.side_panel = test_side_panel_snapshot("plan", "Plan");

        let source_session_id = app.session.id.clone();
        let mut target = Session::create(None, Some("catchup brief".to_string()));
        target.add_message(
            crate::message::Role::User,
            vec![crate::message::ContentBlock::Text {
                text: "Please review the final diff.".to_string(),
                cache_control: None,
            }],
        );
        target.add_message(
            crate::message::Role::Assistant,
            vec![crate::message::ContentBlock::Text {
                text: "The implementation is complete and needs your approval.".to_string(),
                cache_control: None,
            }],
        );
        target.mark_closed();
        target.save().expect("save catchup brief session");
        let target_id = target.id.clone();

        app.begin_in_flight_catchup_resume(PendingCatchupResume {
            target_session_id: target_id.clone(),
            source_session_id: Some(source_session_id),
            queue_position: Some((1, 1)),
            show_brief: true,
        });
        app.maybe_show_catchup_after_history(&target_id);

        assert!(app.in_flight_catchup_resume.is_none());
        assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("catchup"));
        assert_eq!(app.side_panel.pages.len(), 2);
        assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

        let page = app.side_panel.focused_page().expect("missing catchup page");
        assert_eq!(page.id, "catchup");
        assert_eq!(page.file_path, format!("catchup://{}", target_id));
        assert!(page.content.contains("# Catch Up"));
        assert!(page.content.contains("Please review the final diff."));
        assert!(page.content.contains("needs your approval"));

        let persisted = Session::load(&target_id).expect("reload catchup target");
        assert!(!crate::catchup::needs_catchup(
            &target_id,
            persisted.updated_at,
            &persisted.status
        ));
    });
}

#[test]
fn test_help_topic_shows_observe_command_details() {
    let mut app = create_test_app();
    app.input = "/help observe".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/observe`"));
    assert!(msg.content.contains("latest tool call or tool result"));
}

#[test]
fn test_help_topic_shows_splitview_command_details() {
    let mut app = create_test_app();
    app.input = "/help splitview".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/splitview`"));
    assert!(
        msg.content
            .contains("mirrors the current chat in the side panel")
    );
}

#[test]
fn test_help_topic_shows_refactor_command_details() {
    let mut app = create_test_app();
    app.input = "/help refactor".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing help response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/refactor [focus]`"));
    assert!(msg.content.contains("independent read-only subagent"));
}

#[test]
fn test_save_command_bookmarks_session_with_memory_enabled() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.memory_enabled = true;
    app.messages = vec![
        Message::user("u1"),
        Message::assistant_text("a1"),
        Message::user("u2"),
        Message::assistant_text("a2"),
    ];

    app.input = "/save quick-label".to_string();
    app.submit_input();

    assert!(app.session.saved);
    assert_eq!(app.session.save_label.as_deref(), Some("quick-label"));
    let msg = app
        .display_messages()
        .last()
        .expect("missing save response");
    assert!(msg.content.contains("saved as"));
    assert!(msg.content.contains("quick-label"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
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
fn test_btw_command_requires_question() {
    let mut app = create_test_app();
    app.input = "/btw".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing btw error");
    assert_eq!(msg.role, "error");
    assert!(msg.content.contains("Usage: `/btw <question>`"));
}

#[test]
fn test_btw_command_prepares_side_panel_and_hidden_turn() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.input = "/btw what did we decide about config?".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("btw"));
    let page = app.side_panel.focused_page().expect("missing btw page");
    assert_eq!(page.title, "`/btw`");
    assert!(page.content.contains("## Question"));
    assert!(page.content.contains("what did we decide about config?"));
    assert!(page.content.contains("Thinking…"));
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(
        app.hidden_queued_system_messages[0].contains("Question: what did we decide about config?")
    );
    assert!(app.pending_queued_dispatch);

    let msg = app
        .display_messages()
        .last()
        .expect("missing btw status message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Running `/btw`"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_btw_command_in_remote_mode_queues_followup_instead_of_erroring() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_session_id = Some("ses_remote_btw".to_string());
    app.input = "/btw what are we doing?".to_string();
    app.submit_input();

    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("btw"));
    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    assert!(app.pending_queued_dispatch);
    let msg = app
        .display_messages()
        .last()
        .expect("missing remote btw message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("Running `/btw`"));

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_git_command_shows_repo_status_for_working_directory() {
    let repo = create_real_git_repo_fixture();
    std::fs::write(repo.path().join("tracked.txt"), "after\n").expect("update tracked file");

    let mut app = create_test_app();
    app.session.working_dir = Some(repo.path().display().to_string());
    app.input = "/git".to_string();
    app.submit_input();

    let msg = app.display_messages().last().expect("missing git response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("`/git`"));
    assert!(msg.content.contains("```text"));
    assert!(msg.content.contains("## "));
    assert!(msg.content.contains("tracked.txt"));
}

#[test]
fn test_observe_command_enables_transient_page_without_persisting() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.input = "/observe on".to_string();
        app.submit_input();

        assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));
        let page = app.side_panel.focused_page().expect("missing observe page");
        assert_eq!(page.title, "Observe");
        assert_eq!(
            page.source,
            crate::side_panel::SidePanelPageSource::Ephemeral
        );
        assert!(
            page.content
                .contains("Waiting for the next tool call or tool result")
        );

        let persisted = crate::side_panel::snapshot_for_session(&app.session.id)
            .expect("load persisted side panel");
        assert!(persisted.pages.is_empty());
        assert!(persisted.focused_page_id.is_none());
    });
}

#[test]
fn test_splitview_command_enables_transient_page_without_persisting() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.input = "/splitview on".to_string();
        app.submit_input();

        assert_eq!(
            app.side_panel.focused_page_id.as_deref(),
            Some("split_view")
        );
        let page = app
            .side_panel
            .focused_page()
            .expect("missing split view page");
        assert_eq!(page.title, "Split View");
        assert_eq!(
            page.source,
            crate::side_panel::SidePanelPageSource::Ephemeral
        );
        assert!(page.content.contains("Mirror of the current chat"));

        let persisted = crate::side_panel::snapshot_for_session(&app.session.id)
            .expect("load persisted side panel");
        assert!(persisted.pages.is_empty());
        assert!(persisted.focused_page_id.is_none());
    });
}

#[test]
fn test_splitview_command_off_restores_previous_side_panel_page() {
    let mut app = create_test_app();
    app.set_side_panel_snapshot(test_side_panel_snapshot("plan", "Plan"));

    app.input = "/splitview on".to_string();
    app.submit_input();
    assert_eq!(
        app.side_panel.focused_page_id.as_deref(),
        Some("split_view")
    );
    assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

    app.input = "/splitview off".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert!(
        !app.side_panel
            .pages
            .iter()
            .any(|page| page.id == "split_view")
    );
}

#[test]
fn test_splitview_mirrors_chat_and_streaming_text() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::system("System note".to_string()),
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];
    app.bump_display_messages_version();
    app.streaming_text = "Working on the follow-up now...".to_string();
    app.set_split_view_enabled(true, true);

    let page = app
        .side_panel
        .focused_page()
        .expect("missing split view page");
    assert!(page.content.contains("## System"));
    assert!(page.content.contains("## Prompt 1"));
    assert!(page.content.contains("What did we decide?"));
    assert!(page.content.contains("## Response 1"));
    assert!(page.content.contains("We decided to ship it."));
    assert!(page.content.contains("## Live response"));
    assert!(page.content.contains("Working on the follow-up now..."));
}

#[test]
fn test_splitview_does_not_build_cache_while_disabled() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];

    app.bump_display_messages_version();

    assert!(!app.split_view_enabled());
    assert!(app.split_view_markdown.is_empty());
}

#[test]
fn test_splitview_disable_clears_cached_markdown() {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("What did we decide?".to_string()),
        DisplayMessage::assistant("We decided to ship it.".to_string()),
    ];
    app.bump_display_messages_version();
    app.set_split_view_enabled(true, true);

    assert!(!app.split_view_markdown.is_empty());

    app.set_split_view_enabled(false, false);

    assert!(app.split_view_markdown.is_empty());
}

#[test]
fn test_observe_command_off_restores_previous_side_panel_page() {
    let mut app = create_test_app();
    app.set_side_panel_snapshot(test_side_panel_snapshot("plan", "Plan"));

    app.input = "/observe on".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));
    assert!(app.side_panel.pages.iter().any(|page| page.id == "plan"));

    app.input = "/observe off".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert!(!app.side_panel.pages.iter().any(|page| page.id == "observe"));
}

#[test]
fn test_observe_updates_latest_tool_context_only() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let tool_call = crate::message::ToolCall {
        id: "tool_1".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs", "start_line": 1, "end_line": 10}),
        intent: None,
    };
    app.observe_tool_call(&tool_call);

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(
        page.content
            .contains("Latest tool call emitted by the model")
    );
    assert!(page.content.contains("`read`"));
    assert!(page.content.contains("src/main.rs"));

    app.observe_tool_result(&tool_call, "1 use std::path::Path;", false, Some("read"));

    let page = app.side_panel.focused_page().expect("missing observe page");
    let token_label = crate::util::format_approx_token_count(crate::util::estimate_tokens(
        "1 use std::path::Path;",
    ));
    assert!(page.content.contains("Latest tool result added to context"));
    assert!(page.content.contains("Status: completed"));
    assert!(page.content.contains("Returned to context"));
    assert!(page.content.contains(&token_label));
    assert!(page.content.contains("1 use std::path::Path;"));
    assert!(
        !page
            .content
            .contains("Latest tool call emitted by the model")
    );
}

#[test]
fn test_observe_ignores_noise_tools_and_preserves_latest_useful_context() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let read_tool = crate::message::ToolCall {
        id: "tool_read".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs"}),
        intent: None,
    };
    app.observe_tool_result(&read_tool, "fn main() {}", false, Some("read"));
    let before = app
        .side_panel
        .focused_page()
        .expect("missing observe page")
        .content
        .clone();

    let noise_tool = crate::message::ToolCall {
        id: "tool_side_panel".to_string(),
        name: "side_panel".to_string(),
        input: serde_json::json!({"action": "write", "page_id": "plan"}),
        intent: None,
    };
    app.observe_tool_call(&noise_tool);
    app.observe_tool_result(&noise_tool, "ok", false, Some("side_panel"));

    let after = app
        .side_panel
        .focused_page()
        .expect("missing observe page")
        .content
        .clone();
    assert_eq!(after, before);
    assert!(after.contains("fn main() {}"));
    assert!(!after.contains("tool_side_panel"));
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
fn test_fast_default_on_saves_config_and_updates_session() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let mut app = create_fast_test_app();
    app.input = "/fast default on".to_string();

    app.submit_input();

    let cfg = crate::config::Config::load();
    assert_eq!(
        cfg.provider.openai_service_tier.as_deref(),
        Some("priority")
    );
    assert_eq!(app.provider.service_tier().as_deref(), Some("priority"));
    assert_eq!(app.status_notice(), Some("Fast mode: on".to_string()));
    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.content, "Saved OpenAI fast mode: **on**.");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_fast_status_shows_saved_default() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::config::Config::set_openai_service_tier(Some("priority")).expect("save fast default");

    let mut app = create_fast_test_app();
    app.input = "/fast status".to_string();

    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(
        last.content,
        "Fast mode is off.\nCurrent tier: Standard\nSaved default: on (Fast)\nUse `/fast on`, `/fast off`, or `/fast default on|off`."
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_alignment_command_persists_and_applies_immediately() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.set_centered(false);
        app.input = "/alignment centered".to_string();

        app.submit_input();

        let cfg = crate::config::Config::load();
        assert!(cfg.display.centered);
        assert!(app.centered_mode());
        assert_eq!(app.status_notice(), Some("Layout: Centered".to_string()));

        let last = app.display_messages().last().expect("missing response");
        assert_eq!(last.role, "system");
        assert!(
            last.content
                .contains("Saved default alignment: **centered**")
        );
    });
}

#[test]
fn test_alignment_status_shows_current_and_saved_defaults() {
    with_temp_jcode_home(|| {
        crate::config::Config::set_display_centered(false).expect("save alignment default");

        let mut app = create_test_app();
        app.set_centered(true);
        app.input = "/alignment".to_string();

        app.submit_input();

        let last = app.display_messages().last().expect("missing response");
        assert_eq!(last.role, "system");
        assert!(
            last.content
                .contains("Alignment is currently **centered**.")
        );
        assert!(last.content.contains("Saved default: **left-aligned**."));
        assert!(last.content.contains("/alignment centered"));
        assert!(last.content.contains("Alt+C"));
    });
}

#[test]
fn test_alignment_invalid_usage_shows_error() {
    let mut app = create_test_app();
    app.input = "/alignment diagonal".to_string();

    app.submit_input();

    let last = app.display_messages().last().expect("missing response");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("Usage: `/alignment`"));
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
fn test_usage_report_shows_no_connected_providers_when_results_empty() {
    let mut app = create_test_app();
    app.handle_usage_report(Vec::new());

    let msg = app
        .display_messages()
        .last()
        .expect("missing /usage response");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("## Usage"));
    assert!(
        msg.content
            .contains("No providers with OAuth credentials found")
    );
    assert!(msg.content.contains("/login claude"));
    assert!(msg.content.contains("/login openai"));
}

#[test]
fn test_usage_command_requests_usage_report_with_inline_view() {
    let mut app = create_test_app();

    assert!(super::commands::handle_usage_command(&mut app, "/usage"));

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_some());
    assert!(app.usage_report_refreshing);
}

#[test]
fn test_usage_submit_input_requests_usage_report_with_inline_view() {
    let mut app = create_test_app();
    app.input = "/usage".to_string();

    app.submit_input();

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_some());
    assert!(app.display_messages().is_empty());
    assert!(app.usage_report_refreshing);
}

#[test]
fn test_usage_typing_does_not_open_picker_preview() {
    let mut app = create_test_app();

    for c in "/usage".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("type /usage");
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/usage");
    assert!(!app.usage_report_refreshing);
}

#[test]
fn test_usage_enter_requests_report_with_inline_view() {
    let mut app = create_test_app();

    for c in "/usage".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .expect("type /usage");
    }

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("submit /usage");

    assert!(app.inline_interactive_state.is_none());
    assert!(app.usage_overlay.is_none());
    assert!(app.inline_view_state.is_some());
    assert_eq!(app.input(), "");
    assert!(app.usage_report_refreshing);
}

#[test]
fn test_usage_report_pushes_system_message() {
    let mut app = create_test_app();
    app.usage_report_refreshing = true;
    app.handle_usage_report(vec![crate::usage::ProviderUsage {
        provider_name: "OpenAI (ChatGPT)".to_string(),
        limits: vec![crate::usage::UsageLimit {
            name: "5h".to_string(),
            usage_percent: 82.0,
            resets_at: None,
        }],
        extra_info: vec![("plan".to_string(), "pro".to_string())],
        hard_limit_reached: false,
        error: None,
    }]);

    assert!(!app.usage_report_refreshing);
    assert!(app.inline_view_state.is_none());
    let msg = app
        .display_messages()
        .last()
        .expect("missing usage report message");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("## Usage"));
    assert!(msg.content.contains("### OpenAI (ChatGPT)"));
    assert!(msg.content.contains("**5h**"));
    assert!(msg.content.contains("82%"));
    assert!(msg.content.contains("plan: pro"));
}

#[test]
fn test_usage_with_suffix_does_not_open_picker_preview() {
    let mut app = create_test_app();

    for c in "/usage open".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/usage open");
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
    with_temp_jcode_home(|| {
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

        assert!(app.account_picker_overlay.is_none());
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("/account openai should open the inline account picker");
        assert_eq!(picker.kind, crate::tui::PickerKind::Account);
        assert!(picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                    ref provider_id,
                    ..
                }) if provider_id == "openai"
            )
        }));
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "new account")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "replace account")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "account center")
        );
    });
}

#[test]
fn test_account_command_opens_account_picker() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "claude_acc".to_string(),
            refresh: "claude_ref".to_string(),
            expires: now_ms + 60_000,
            email: Some("claude@example.com".to_string()),
            subscription_type: Some("pro".to_string()),
        })
        .unwrap();

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

        assert!(app.account_picker_overlay.is_none());
        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("/account should open the inline account picker");
        assert!(picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                    ref provider_id,
                    ref label
                }) if provider_id == "claude" && label == "claude-1"
            )
        }));
        assert!(picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                    ref provider_id,
                    ..
                }) if provider_id == "openai"
            )
        }));
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "new Claude account")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "new OpenAI account")
        );
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "account center")
        );
    });
}

#[test]
fn test_account_picker_supports_arrow_and_vim_navigation() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "first".to_string(),
            access_token: "acc1".to_string(),
            refresh_token: "ref1".to_string(),
            id_token: None,
            account_id: Some("acct_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("first@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "second".to_string(),
            access_token: "acc2".to_string(),
            refresh_token: "ref2".to_string(),
            id_token: None,
            account_id: Some("acct_2".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("second@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/account openai".to_string();
        app.submit_input();

        let initial_selected = app
            .inline_interactive_state
            .as_ref()
            .expect("inline account picker should open")
            .selected;

        app.handle_key(KeyCode::Down, KeyModifiers::empty())
            .unwrap();
        let after_arrow = app.inline_interactive_state.as_ref().unwrap().selected;
        assert_eq!(after_arrow, initial_selected + 1);

        app.handle_key(KeyCode::Char('j'), KeyModifiers::empty())
            .unwrap();
        let after_vim = app.inline_interactive_state.as_ref().unwrap().selected;
        assert_eq!(after_vim, after_arrow + 1);

        app.handle_key(KeyCode::Char('k'), KeyModifiers::empty())
            .unwrap();
        assert_eq!(
            app.inline_interactive_state.as_ref().unwrap().selected,
            after_arrow
        );
    });
}

#[test]
fn test_account_picker_preview_from_input_filters_accounts() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "first".to_string(),
            access_token: "acc1".to_string(),
            refresh_token: "ref1".to_string(),
            id_token: None,
            account_id: Some("acct_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("first@example.com".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "second".to_string(),
            access_token: "acc2".to_string(),
            refresh_token: "ref2".to_string(),
            id_token: None,
            account_id: Some("acct_2".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("second@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        for c in "/account openai sec".chars() {
            app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
                .unwrap();
        }

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("account preview should open");
        assert!(picker.preview, "account picker should stay in preview mode");
        assert_eq!(picker.kind, crate::tui::PickerKind::Account);
        assert_eq!(picker.filter, "sec");
        assert!(app.account_picker_overlay.is_none());
        assert_eq!(app.input(), "/account openai sec");
    });
}

#[test]
fn test_account_picker_preview_stays_closed_for_explicit_subcommands() {
    let mut app = create_test_app();

    for c in "/account openai settings".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.input(), "/account openai settings");
}

#[test]
fn test_account_command_combines_claude_and_openai_accounts() {
    with_temp_jcode_home(|| {
        let now_ms = chrono::Utc::now().timestamp_millis();

        crate::auth::claude::upsert_account(crate::auth::claude::AnthropicAccount {
            label: "claude-1".to_string(),
            access: "claude_acc".to_string(),
            refresh: "claude_ref".to_string(),
            expires: now_ms + 60_000,
            email: Some("claude@example.com".to_string()),
            subscription_type: Some("pro".to_string()),
        })
        .unwrap();
        crate::auth::codex::upsert_account(crate::auth::codex::OpenAiAccount {
            label: "openai-1".to_string(),
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            id_token: None,
            account_id: Some("acct_openai_1".to_string()),
            expires_at: Some(now_ms + 60_000),
            email: Some("openai@example.com".to_string()),
        })
        .unwrap();

        let mut app = create_test_app();
        app.input = "/account".to_string();
        app.submit_input();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("inline account picker should open");
        assert!(picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                    ref provider_id,
                    ref label
                }) if provider_id == "claude" && label == "claude-1"
            )
        }));
        assert!(picker.entries.iter().any(|entry| {
            matches!(
                entry.action,
                crate::tui::PickerAction::Account(crate::tui::AccountPickerAction::Switch {
                    ref provider_id,
                    ref label
                }) if provider_id == "openai" && label == "openai-1"
            )
        }));
        assert!(
            picker
                .entries
                .iter()
                .any(|entry| entry.name == "account center")
        );
    });
}

#[cfg(unix)]
#[test]
fn test_account_command_uses_fast_auth_snapshot_without_running_cursor_status() {
    use std::os::unix::fs::PermissionsExt;

    with_temp_jcode_home(|| {
        let prev_cursor_cli_path = std::env::var_os("JCODE_CURSOR_CLI_PATH");
        let temp = tempfile::TempDir::new().expect("create temp dir");
        let marker = temp.path().join("cursor-status-ran");
        let script = temp.path().join("cursor-agent-mock");

        std::fs::write(
            &script,
            format!("#!/bin/sh\necho ran > \"{}\"\nexit 0\n", marker.display()),
        )
        .expect("write mock cursor agent");
        let mut permissions = std::fs::metadata(&script)
            .expect("stat mock cursor agent")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).expect("chmod mock cursor agent");

        let mut app = create_test_app();

        crate::env::set_var("JCODE_CURSOR_CLI_PATH", &script);
        crate::auth::AuthStatus::invalidate_cache();
        let _ = std::fs::remove_file(&marker);

        app.input = "/account".to_string();
        app.submit_input();

        assert!(app.inline_interactive_state.is_some());
        assert!(
            !marker.exists(),
            "/account should not execute `cursor-agent status` on open"
        );

        match prev_cursor_cli_path {
            Some(value) => crate::env::set_var("JCODE_CURSOR_CLI_PATH", value),
            None => crate::env::remove_var("JCODE_CURSOR_CLI_PATH"),
        }
        crate::auth::AuthStatus::invalidate_cache();
    });
}

#[test]
fn test_account_switch_shorthand_switches_openai_account_by_label() {
    with_temp_jcode_home(|| {
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
                Some("openai-1")
            );
        });
    });
}

#[test]
fn test_account_picker_prompt_new_openai_label_cancel_clears_prompt() {
    let mut app = create_test_app();
    app.prompt_new_account_label(crate::tui::account_picker::AccountProviderKind::OpenAi);

    assert!(matches!(
        app.pending_account_input,
        Some(super::auth::PendingAccountInput::NewAccountLabel { ref provider_id, .. }) if provider_id == "openai"
    ));

    app.input = "/cancel".to_string();
    app.submit_input();

    assert!(app.pending_account_input.is_none());
    assert!(app.pending_login.is_none());
}

#[test]
fn test_login_command_opens_inline_login_picker() {
    let mut app = create_test_app();
    app.input = "/login".to_string();
    app.submit_input();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("/login should open inline login picker");
    assert_eq!(picker.kind, crate::tui::PickerKind::Login);
    assert!(app.pending_login.is_none());
}

#[test]
fn test_account_openai_compatible_settings_renders_provider_settings() {
    let mut app = create_test_app();
    app.input = "/account openai-compatible settings".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing settings output");
    assert_eq!(msg.role, "system");
    assert!(msg.content.contains("OpenAI-compatible"));
    assert!(msg.content.contains("API base"));
    assert!(msg.content.contains("default-model"));
}

#[test]
fn test_account_default_provider_command_saves_config() {
    let _guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    app.input = "/account default-provider openai".to_string();
    app.submit_input();

    let cfg = crate::config::Config::load();
    assert_eq!(cfg.provider.default_provider.as_deref(), Some("openai"));
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
fn test_improve_command_starts_improvement_loop() {
    let mut app = create_test_app();
    app.input = "/improve".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::ImproveRun));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::ImproveRun)
    );
    assert!(app.is_processing());

    let msg = app.session.messages.last().expect("missing improve prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering improvement mode for this repository")
                && text.contains("write a concise ranked todo list using `todo`")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing improve launch notice");
    assert!(display.content.contains("Starting improvement loop"));
}

#[test]
fn test_improve_plan_command_is_plan_only_and_accepts_focus() {
    let mut app = create_test_app();
    app.input = "/improve plan startup performance".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::ImprovePlan));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::ImprovePlan)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing improve plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("improvement planning mode")
                && text.contains("This is plan-only mode")
                && text.contains("Focus area: startup performance")
    ));
}

#[test]
fn test_improve_status_summarizes_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[
                crate::todo::TodoItem {
                    id: "one".to_string(),
                    content: "Profile startup path".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "two".to_string(),
                    content: "Add regression test".to_string(),
                    status: "completed".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
            ],
        )
        .expect("save todos");

        app.improve_mode = Some(ImproveMode::ImproveRun);
        app.input = "/improve status".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing improve status");
        assert!(msg.content.contains("Improve status"));
        assert!(
            msg.content
                .contains("1 incomplete · 1 completed · 0 cancelled")
        );
        assert!(msg.content.contains("Profile startup path"));
    });
}

#[test]
fn test_improve_stop_without_active_run_reports_idle() {
    let mut app = create_test_app();
    app.session.improve_mode = None;
    app.input = "/improve stop".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing improve stop idle message");
    assert!(msg.content.contains("No active improve loop to stop"));
}

#[test]
fn test_improve_stop_queues_stop_prompt_and_clears_mode() {
    let mut app = create_test_app();
    app.improve_mode = Some(ImproveMode::ImproveRun);
    app.session.improve_mode = Some(crate::session::SessionImproveMode::ImproveRun);
    app.input = "/improve stop".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, None);
    assert_eq!(app.session.improve_mode, None);
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing improve stop prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("Stop improvement mode after the current safe point")
    ));
}

#[test]
fn test_improve_resume_requires_saved_mode() {
    let mut app = create_test_app();
    app.input = "/improve resume".to_string();
    app.submit_input();

    let msg = app
        .display_messages()
        .last()
        .expect("missing improve resume idle message");
    assert!(msg.content.contains("No saved improve run found"));
}

#[test]
fn test_improve_resume_uses_saved_mode_and_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.session.improve_mode = Some(crate::session::SessionImproveMode::ImproveRun);
        app.session.save().expect("save session");
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "resume1".to_string(),
                content: "Refactor command parsing".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.input = "/improve resume".to_string();
        app.submit_input();

        assert_eq!(app.improve_mode, Some(ImproveMode::ImproveRun));
        assert_eq!(
            app.session.improve_mode,
            Some(crate::session::SessionImproveMode::ImproveRun)
        );
        assert!(app.is_processing());

        let msg = app
            .session
            .messages
            .last()
            .expect("missing improve resume prompt");
        assert!(matches!(
            &msg.content[0],
            ContentBlock::Text { text, .. }
                if text.contains("Resume improvement mode")
                    && text.contains("Refactor command parsing")
        ));
    });
}

#[test]
fn test_improve_mode_persists_in_session_file() {
    with_temp_jcode_home(|| {
        let mut session = crate::session::Session::create(None, None);
        session.improve_mode = Some(crate::session::SessionImproveMode::ImprovePlan);
        let session_id = session.id.clone();
        session.save().expect("save session");

        let loaded = crate::session::Session::load(&session_id).expect("load session");
        assert_eq!(
            loaded.improve_mode,
            Some(crate::session::SessionImproveMode::ImprovePlan)
        );
    });
}

#[test]
fn test_refactor_command_starts_refactor_loop() {
    let mut app = create_test_app();
    app.input = "/refactor".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::RefactorRun));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::RefactorRun)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing refactor prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("You are entering refactor mode for this repository")
                && text.contains("use the `subagent` tool exactly once")
    ));

    let display = app
        .display_messages()
        .last()
        .expect("missing refactor launch notice");
    assert!(display.content.contains("Starting refactor loop"));
}

#[test]
fn test_refactor_plan_command_is_plan_only_and_accepts_focus() {
    let mut app = create_test_app();
    app.input = "/refactor plan command parsing".to_string();
    app.submit_input();

    assert_eq!(app.improve_mode, Some(ImproveMode::RefactorPlan));
    assert_eq!(
        app.session.improve_mode,
        Some(crate::session::SessionImproveMode::RefactorPlan)
    );
    assert!(app.is_processing());

    let msg = app
        .session
        .messages
        .last()
        .expect("missing refactor plan prompt");
    assert!(matches!(
        &msg.content[0],
        ContentBlock::Text { text, .. }
            if text.contains("refactor planning mode")
                && text.contains("This is plan-only mode")
                && text.contains("Focus area: command parsing")
    ));
}

#[test]
fn test_refactor_status_summarizes_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[
                crate::todo::TodoItem {
                    id: "one".to_string(),
                    content: "Split giant module".to_string(),
                    status: "in_progress".to_string(),
                    priority: "high".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
                crate::todo::TodoItem {
                    id: "two".to_string(),
                    content: "Run review subagent".to_string(),
                    status: "completed".to_string(),
                    priority: "medium".to_string(),
                    blocked_by: Vec::new(),
                    assigned_to: None,
                },
            ],
        )
        .expect("save todos");

        app.improve_mode = Some(ImproveMode::RefactorRun);
        app.input = "/refactor status".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing refactor status");
        assert!(msg.content.contains("Refactor status"));
        assert!(
            msg.content
                .contains("1 incomplete · 1 completed · 0 cancelled")
        );
        assert!(msg.content.contains("Split giant module"));
    });
}

#[test]
fn test_refactor_resume_uses_saved_mode_and_current_todos() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.session.improve_mode = Some(crate::session::SessionImproveMode::RefactorRun);
        app.session.save().expect("save session");
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "resume1".to_string(),
                content: "Extract review prompt builder".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.input = "/refactor resume".to_string();
        app.submit_input();

        assert_eq!(app.improve_mode, Some(ImproveMode::RefactorRun));
        assert_eq!(
            app.session.improve_mode,
            Some(crate::session::SessionImproveMode::RefactorRun)
        );
        assert!(app.is_processing());

        let msg = app
            .session
            .messages
            .last()
            .expect("missing refactor resume prompt");
        assert!(matches!(
            &msg.content[0],
            ContentBlock::Text { text, .. }
                if text.contains("Resume refactor mode")
                    && text.contains("Extract review prompt builder")
        ));
    });
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

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));

    app.accumulate_streaming_output_tokens(10, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);
    app.accumulate_streaming_output_tokens(30, &mut seen);

    assert_eq!(app.streaming_total_output_tokens, 30);
    assert_eq!(app.streaming_tps_observed_output_tokens, 30);
    assert!(app.streaming_tps_observed_elapsed >= Duration::from_secs(9));
    assert_eq!(seen, 30);
}

#[test]
fn test_accumulate_streaming_output_tokens_ignores_hidden_output_phase() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.accumulate_streaming_output_tokens(20, &mut seen);
    assert_eq!(app.streaming_total_output_tokens, 0);
    assert_eq!(app.streaming_tps_observed_output_tokens, 0);
    assert_eq!(seen, 20);

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));
    app.accumulate_streaming_output_tokens(60, &mut seen);

    assert_eq!(app.streaming_total_output_tokens, 40);
    assert_eq!(app.streaming_tps_observed_output_tokens, 40);
    assert_eq!(seen, 60);
}

#[test]
fn test_compute_streaming_tps_uses_latest_observed_snapshot_instead_of_current_repaint_time() {
    let mut app = create_test_app();
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(20));
    app.streaming_tps_observed_output_tokens = 40;
    app.streaming_tps_observed_elapsed = Duration::from_secs(10);

    let tps = app.compute_streaming_tps().expect("tps");
    assert!(tps > 3.9 && tps < 4.1, "unexpected tps: {tps}");
}

#[test]
fn test_compute_streaming_tps_does_not_decay_on_redundant_usage_snapshots() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(10));
    app.accumulate_streaming_output_tokens(40, &mut seen);
    let initial_tps = app.compute_streaming_tps().expect("initial tps");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(30));
    app.accumulate_streaming_output_tokens(40, &mut seen);

    let tps = app.compute_streaming_tps().expect("tps");
    assert!(
        initial_tps > 3.9 && initial_tps < 4.1,
        "unexpected initial tps: {initial_tps}"
    );
    assert!(
        tps > 3.9 && tps < 4.1,
        "unexpected tps after redundant snapshot: {tps}"
    );
}

#[test]
fn test_compute_streaming_tps_bursty_stream_simulation_stays_constant_between_real_updates() {
    let mut app = create_test_app();
    let mut seen = 0;

    app.streaming_tps_collect_output = true;

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(2));
    app.accumulate_streaming_output_tokens(10, &mut seen);
    let tps_after_first_burst = app.compute_streaming_tps().expect("tps after first burst");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(5));
    app.accumulate_streaming_output_tokens(10, &mut seen);
    let tps_after_idle_gap = app.compute_streaming_tps().expect("tps after idle gap");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(6));
    app.accumulate_streaming_output_tokens(30, &mut seen);
    let tps_after_second_burst = app.compute_streaming_tps().expect("tps after second burst");

    app.streaming_tps_start = Some(Instant::now() - Duration::from_secs(9));
    app.accumulate_streaming_output_tokens(30, &mut seen);
    let tps_after_second_idle_gap = app
        .compute_streaming_tps()
        .expect("tps after second idle gap");

    assert!(
        tps_after_first_burst > 4.9 && tps_after_first_burst < 5.1,
        "unexpected first burst tps: {tps_after_first_burst}"
    );
    assert!(
        (tps_after_idle_gap - tps_after_first_burst).abs() < 0.01,
        "tps changed without new tokens: first={tps_after_first_burst} idle={tps_after_idle_gap}"
    );
    assert!(
        tps_after_second_burst > 4.9 && tps_after_second_burst < 5.1,
        "unexpected second burst tps: {tps_after_second_burst}"
    );
    assert!(
        (tps_after_second_idle_gap - tps_after_second_burst).abs() < 0.01,
        "tps changed without new tokens: second={tps_after_second_burst} idle={tps_after_second_idle_gap}"
    );
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
fn test_handle_key_shift_slash_inserts_question_mark() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char('/'), KeyModifiers::SHIFT)
        .unwrap();

    assert_eq!(app.input(), "?");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_handle_key_event_shift_slash_inserts_question_mark() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();

    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Char('/'),
        KeyModifiers::SHIFT,
        KeyEventKind::Press,
    ));

    assert_eq!(app.input(), "?");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_super_space_toggles_next_prompt_new_session_routing() {
    let mut app = create_test_app();

    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .unwrap();
    assert!(app.route_next_prompt_to_new_session);
    assert_eq!(
        app.status_notice(),
        Some("Next prompt → new session".to_string())
    );

    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .unwrap();
    assert!(!app.route_next_prompt_to_new_session);
    assert_eq!(
        app.status_notice(),
        Some("Next-prompt new session canceled".to_string())
    );
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
fn test_ctrl_l_without_focusable_pane_does_not_clear_session() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Off;
    app.input = "draft message".to_string();
    app.cursor_pos = app.input.len();
    app.display_messages = vec![DisplayMessage::system("keep chat".to_string())];
    app.bump_display_messages_version();

    app.handle_key(KeyCode::Char('l'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "draft message");
    assert_eq!(app.cursor_pos(), "draft message".len());
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(app.display_messages()[0].content, "keep chat");
    assert!(!app.diagram_focus);
    assert!(!app.diff_pane_focus);
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
fn test_cycle_diagram_resets_view_to_fit() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_focus = true;
    app.diagram_zoom = 140;
    app.diagram_scroll_x = 12;
    app.diagram_scroll_y = 7;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x1, 100, 80, None);
    crate::tui::mermaid::register_active_diagram(0x2, 120, 90, None);

    app.cycle_diagram(1);

    assert_eq!(app.diagram_index, 1);
    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_resize_resets_diagram_and_side_panel_diagram_view_to_fit() {
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_zoom = 130;
    app.diagram_scroll_x = 9;
    app.diagram_scroll_y = 4;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "```mermaid\nflowchart LR\nA-->B\n```".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.diff_pane_scroll_x = 17;

    assert!(app.should_redraw_after_resize());
    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);
    assert_eq!(app.diff_pane_scroll_x, 0);
}

#[test]
fn test_side_panel_visibility_change_resets_diagram_fit_context() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0xabc, 900, 450, None);

    app.normalize_diagram_state();
    assert_eq!(app.last_visible_diagram_hash, Some(0xabc));

    app.diagram_zoom = 150;
    app.diagram_scroll_x = 8;
    app.diagram_scroll_y = 3;
    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("side".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "side".to_string(),
            title: "Side".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    });

    assert_eq!(app.diagram_zoom, 100);
    assert_eq!(app.diagram_scroll_x, 0);
    assert_eq!(app.diagram_scroll_y, 0);
    assert_eq!(app.last_visible_diagram_hash, None);

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot::default());
    assert_eq!(app.last_visible_diagram_hash, Some(0xabc));

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_goal_side_panel_focus_updates_status_notice() {
    let mut app = create_test_app();

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("goals".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "goals".to_string(),
            title: "Goals".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Goals".to_string(),
            updated_at_ms: 1,
        }],
    });
    assert_eq!(app.status_notice(), Some("Goals".to_string()));

    app.set_side_panel_snapshot(crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("goal.ship-mobile-mvp".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "goal.ship-mobile-mvp".to_string(),
            title: "Goal: Ship mobile MVP".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Goal: Ship mobile MVP".to_string(),
            updated_at_ms: 2,
        }],
    });
    assert_eq!(
        app.status_notice(),
        Some("Goal: Ship mobile MVP".to_string())
    );
}

#[test]
fn test_side_panel_same_page_update_preserves_scroll_position() {
    let mut app = create_test_app();
    app.diff_pane_scroll = 14;
    app.diff_pane_scroll_x = 3;

    let first = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n\nVersion 1".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.set_side_panel_snapshot(first);
    app.diff_pane_scroll = 14;
    app.diff_pane_scroll_x = 3;

    let second = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "plan.md".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n\nVersion 2".to_string(),
            updated_at_ms: 2,
        }],
    };
    app.set_side_panel_snapshot(second);

    assert_eq!(app.diff_pane_scroll, 14);
    assert_eq!(app.diff_pane_scroll_x, 3);
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
fn test_workspace_info_widget_appears_in_visual_debug_frame_when_enabled() {
    let _render_lock = scroll_render_test_lock();
    crate::tui::workspace_client::reset_for_tests();

    let mut app = create_test_app();
    app.centered = true;
    app.display_messages = vec![
        DisplayMessage::system("Workspace widget render test".to_string()),
        DisplayMessage::assistant("Short content keeps room for info widgets.".to_string()),
    ];
    app.bump_display_messages_version();

    let current_session = app.session.id.clone();
    crate::tui::workspace_client::enable(
        Some(current_session.as_str()),
        &[current_session.clone(), "workspace_peer".to_string()],
    );

    crate::tui::visual_debug::enable();
    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");

    let frame = crate::tui::visual_debug::latest_frame().expect("frame capture");
    let widget = frame
        .layout
        .widget_placements
        .iter()
        .find(|placement| placement.kind == "workspace")
        .expect("workspace widget placement");

    assert_eq!(widget.side, "right");
    assert!(
        widget.rect.width > 0,
        "workspace widget width should be non-zero"
    );
    assert!(
        widget.rect.height > 0,
        "workspace widget height should be non-zero"
    );
    assert!(
        frame
            .info_widgets
            .as_ref()
            .expect("info widget capture")
            .placements
            .iter()
            .any(|placement| placement.kind == "workspace"),
        "workspace widget should be present in info widget capture"
    );

    crate::tui::visual_debug::disable();
    crate::tui::workspace_client::reset_for_tests();
}

#[test]
fn test_mouse_scroll_over_diff_pane_scrolls_side_panel_without_changing_focus() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.diff_pane_scroll = 5;
    app.diff_pane_focus = false;
    app.diff_pane_auto_scroll = true;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diff_pane_scroll, 6);
    assert!(!app.diff_pane_focus);
    assert!(!app.diff_pane_auto_scroll);
}

#[test]
fn test_mouse_scroll_animation_preserves_side_pane_scroll_sensitivity() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.diff_pane_scroll = 5;
    app.diff_pane_auto_scroll = true;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.diff_pane_scroll, 6, "first frame should move one line");

    let _ = crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(app.diff_pane_scroll, 7);

    crate::tui::app::local::handle_tick(&mut app);
    assert_eq!(
        app.diff_pane_scroll, 8,
        "one wheel notch should still total three lines"
    );
}

#[test]
fn test_mouse_scroll_over_tool_side_panel_scrolls_shared_right_pane_without_changing_focus() {
    let _render_lock = scroll_render_test_lock();
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
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        !scroll_only,
        "side-panel wheel scroll should request an immediate redraw"
    );
    assert_eq!(app.diff_pane_scroll, 6);
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
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });
    assert!(
        !scroll_only,
        "side-panel wheel scroll should still keep chat focus while redrawing immediately"
    );
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
            source: crate::side_panel::SidePanelPageSource::Managed,
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
    let diff_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|l| l.diff_pane_area)
        .expect("expected side panel area after render");
    assert!(before.contains("side-scroll-01"));

    let _scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: diff_area.x + diff_area.width / 2,
        row: diff_area.y + diff_area.height.saturating_sub(2).min(3),
        modifiers: KeyModifiers::empty(),
    });
    assert_eq!(app.diff_pane_scroll, 1);

    let after = render_and_snap(&app, &mut terminal);
    assert_eq!(crate::tui::ui::last_diff_pane_effective_scroll(), 1);
    assert_ne!(
        before, after,
        "hover scrolling should repaint the side panel"
    );
    assert!(after.contains("side-scroll-02"));
    assert!(after.contains("side-scroll-03"));
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
            source: crate::side_panel::SidePanelPageSource::Managed,
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
fn test_side_panel_uses_left_splitter_instead_of_rounded_box() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "alpha\nbeta\ngamma".to_string(),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    let diff_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|layout| layout.diff_pane_area)
        .expect("expected side panel area after render");
    let buf = terminal.backend().buffer();

    assert_eq!(buf[(diff_area.x, diff_area.y)].symbol(), "│");
    assert_eq!(buf[(diff_area.x, diff_area.y + 1)].symbol(), "│");
    assert!(text.contains("side Plan 1/1"), "rendered text: {text}");
}

#[test]
fn test_pinned_content_uses_left_splitter_instead_of_rounded_box() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Pinned;
    app.display_messages = vec![DisplayMessage {
        role: "tool".to_string(),
        content: "wrote src/demo.rs".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "tool-1".to_string(),
            name: "write".to_string(),
            input: serde_json::json!({
                "file_path": "src/demo.rs",
                "content": "fn demo() {}\n"
            }),
            intent: None,
        }),
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(80, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    let diff_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|layout| layout.diff_pane_area)
        .expect("expected pinned pane area after render");
    let buf = terminal.backend().buffer();

    assert_eq!(buf[(diff_area.x, diff_area.y)].symbol(), "│");
    assert_eq!(buf[(diff_area.x, diff_area.y + 1)].symbol(), "│");
    assert!(text.contains("pinned"), "rendered text: {text}");
}

#[test]
fn test_file_diff_uses_left_splitter_instead_of_rounded_box() {
    let _lock = scroll_render_test_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let file_path = temp.path().join("demo.rs");
    std::fs::write(&file_path, "fn demo() {}\n").expect("write demo file");

    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;
    app.display_messages = vec![DisplayMessage {
        role: "tool".to_string(),
        content: "updated demo.rs".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(crate::message::ToolCall {
            id: "tool-1".to_string(),
            name: "write".to_string(),
            input: serde_json::json!({
                "file_path": file_path.display().to_string(),
                "content": "fn demo() {\n    println!(\"hi\");\n}\n"
            }),
            intent: None,
        }),
    }];
    app.bump_display_messages_version();

    let backend = ratatui::backend::TestBackend::new(100, 18);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    let diff_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|layout| layout.diff_pane_area)
        .expect("expected file diff pane area after render");
    let buf = terminal.backend().buffer();

    assert_eq!(buf[(diff_area.x, diff_area.y)].symbol(), "│");
    assert_eq!(buf[(diff_area.x, diff_area.y + 1)].symbol(), "│");
    assert!(text.contains("demo.rs"), "rendered text: {text}");
}

#[test]
fn test_side_diagram_uses_left_splitter_instead_of_rounded_box() {
    let _lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diagram_mode = crate::config::DiagramDisplayMode::Pinned;
    app.diagram_pane_enabled = true;
    app.diagram_pane_position = crate::config::DiagramPanePosition::Side;

    crate::tui::mermaid::clear_active_diagrams();
    crate::tui::mermaid::register_active_diagram(0x444, 900, 450, Some("side".to_string()));

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let text = render_and_snap(&app, &mut terminal);

    let diagram_area = crate::tui::ui::last_layout_snapshot()
        .and_then(|layout| layout.diagram_area)
        .expect("expected side diagram area after render");
    let buf = terminal.backend().buffer();

    assert_eq!(buf[(diagram_area.x, diagram_area.y)].symbol(), "│");
    assert_eq!(buf[(diagram_area.x, diagram_area.y + 1)].symbol(), "│");
    assert!(text.contains("pinned 1/1"), "rendered text: {text}");

    crate::tui::mermaid::clear_active_diagrams();
}

#[test]
fn test_tool_side_panel_focus_supports_horizontal_pan_keys() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    assert!(app.handle_diagram_ctrl_key(KeyCode::Char('l'), false));
    assert!(app.diff_pane_focus);

    app.handle_key(KeyCode::Right, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diff_pane_scroll_x, 4);
    assert!(app.input.is_empty());

    app.handle_key(KeyCode::Left, KeyModifiers::empty())
        .unwrap();
    assert_eq!(app.diff_pane_scroll_x, 0);
}

#[test]
fn test_mouse_horizontal_scroll_over_tool_side_panel_pans_without_focus_change() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_scroll_x = 0;
    app.diff_pane_focus = false;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: "".to_string(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollRight,
        column: 45,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        !scroll_only,
        "side-panel horizontal pan should request an immediate redraw"
    );
    assert_eq!(app.diff_pane_scroll_x, 3);
    assert!(!app.diff_pane_focus);
}

#[test]
fn test_mouse_scroll_events_are_classified_as_scroll_only() {
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::File;

    crate::tui::ui::record_layout_snapshot(
        Rect::new(0, 0, 40, 20),
        None,
        Some(Rect::new(40, 0, 20, 20)),
        None,
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
fn test_handterm_native_scroll_command_updates_chat_offset() {
    let mut app = create_test_app();
    let (_scroll_app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("draw failed");
    crate::tui::ui::record_layout_snapshot(Rect::new(0, 0, 50, 12), None, None, None);

    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    app.apply_handterm_native_scroll(super::handterm_native_scroll::HostToApp::Scroll {
        pane: super::handterm_native_scroll::PaneKind::Chat,
        delta: -2,
    });
    assert_eq!(app.scroll_offset, 4);

    app.apply_handterm_native_scroll(super::handterm_native_scroll::HostToApp::Scroll {
        pane: super::handterm_native_scroll::PaneKind::Chat,
        delta: 3,
    });
    assert_eq!(app.scroll_offset, 7);
}

#[cfg(unix)]
#[test]
fn test_handterm_native_scroll_client_roundtrips_over_socket() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    let _lock = crate::storage::lock_test_env();
    let dir = tempfile::tempdir().expect("tempdir");
    let socket_path = dir.path().join("handterm-scroll.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    unsafe {
        std::env::set_var("HANDTERM_NATIVE_SCROLL_SOCKET", &socket_path);
    }

    let mut client = super::handterm_native_scroll::HandtermNativeScrollClient::connect_from_env()
        .expect("native scroll client should connect from env");
    let (mut server, _) = listener.accept().expect("accept client");
    server
        .set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set read timeout");

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    let _ = render_and_snap(&app, &mut terminal);

    client.sync_from_app(&app);

    let mut buf = [0u8; 4096];
    let n = server.read(&mut buf).expect("read pane snapshot");
    let line = std::str::from_utf8(&buf[..n]).expect("utf8 snapshot");
    assert!(line.contains("pane_snapshot"));
    assert!(line.contains("chat"));
    assert!(line.contains("\"position\":6"));

    server
        .write_all(b"{\"type\":\"scroll\",\"pane\":\"chat\",\"delta\":-2}\n")
        .expect("write host scroll command");

    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let command = runtime
        .block_on(async {
            tokio::time::timeout(Duration::from_secs(1), client.recv())
                .await
                .expect("timeout waiting for scroll command")
        })
        .expect("scroll command should arrive");

    app.apply_handterm_native_scroll(command);
    assert_eq!(app.scroll_offset, 4);

    unsafe {
        std::env::remove_var("HANDTERM_NATIVE_SCROLL_SOCKET");
    }
}

#[test]
fn test_mouse_scroll_help_overlay_updates_help_scroll() {
    let mut app = create_test_app();
    app.help_scroll = Some(5);

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "help overlay mouse wheel should be scroll-only"
    );
    assert_eq!(app.help_scroll, Some(6));

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.help_scroll, Some(5));
}

#[test]
fn test_mouse_scroll_changelog_overlay_updates_changelog_scroll() {
    let mut app = create_test_app();
    app.changelog_scroll = Some(2);

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(
        scroll_only,
        "changelog overlay mouse wheel should be scroll-only"
    );
    assert_eq!(app.changelog_scroll, Some(1));

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.changelog_scroll, Some(2));
}

#[test]
fn test_mouse_scroll_over_unfocused_diagram_does_not_resize_pane() {
    let _render_lock = scroll_render_test_lock();
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
        None,
    );

    let scroll_only = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 90,
        row: 10,
        modifiers: KeyModifiers::empty(),
    });

    assert!(scroll_only);
    assert_eq!(app.diagram_pane_ratio, 40);
    assert_eq!(app.diagram_pane_ratio_from, 40);
    assert_eq!(app.diagram_pane_ratio_target, 40);
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

#[test]
fn test_top_level_command_suggestions_include_config_and_subscription() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/con");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/config"));
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/context"));

    let suggestions = app.get_suggestions_for("/ali");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/alignment"));

    let suggestions = app.get_suggestions_for("/sub");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/subscription"));
}

#[test]
fn test_top_level_command_suggestions_include_catchup_and_back() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/cat");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/catchup"));

    let suggestions = app.get_suggestions_for("/bac");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/back"));

    let suggestions = app.get_suggestions_for("/gi");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/git"));
}

#[test]
fn test_help_topic_suggestions_are_contextual() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/help fi");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/help fix")
    );
}

#[test]
fn test_help_topic_suggestions_include_catchup_topics() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/help cat");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/help catchup"));

    let suggestions = app.get_suggestions_for("/help bac");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/help back"));
}

#[test]
fn test_context_command_reports_session_context_snapshot() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.memory_enabled = true;
        app.swarm_enabled = true;
        app.queue_mode = true;
        app.active_skill = Some("debug".to_string());
        app.queued_messages.push("queued follow-up".to_string());
        app.pending_images
            .push(("image/png".to_string(), "abc".to_string()));
        app.side_panel = crate::side_panel::SidePanelSnapshot {
            focused_page_id: Some("goals".to_string()),
            pages: vec![crate::side_panel::SidePanelPage {
                id: "goals".to_string(),
                title: "Goals".to_string(),
                file_path: "".to_string(),
                format: crate::side_panel::SidePanelPageFormat::Markdown,
                source: crate::side_panel::SidePanelPageSource::Managed,
                content: "goal details".to_string(),
                updated_at_ms: 0,
            }],
        };
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "one".to_string(),
                content: "Inspect context summary".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.input = "/context".to_string();
        app.submit_input();

        let msg = app
            .display_messages()
            .last()
            .expect("missing context report");
        assert_eq!(msg.title.as_deref(), Some("Context"));
        assert!(msg.content.contains("# Session Context"));
        assert!(msg.content.contains("## Prompt / Context Composition"));
        assert!(msg.content.contains("## Compaction"));
        assert!(msg.content.contains("## Session State"));
        assert!(msg.content.contains("## Todos"));
        assert!(msg.content.contains("## Side Panel"));
        assert!(msg.content.contains("Inspect context summary"));
        assert!(msg.content.contains("active skill: debug"));
        assert!(msg.content.contains("queue mode: on"));
    });
}

#[test]
fn test_nested_command_suggestions_filter_partial_suffixes() {
    let app = create_test_app();

    let suggestions = app.get_suggestions_for("/config ed");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/config edit")
    );

    let suggestions = app.get_suggestions_for("/alignment ce");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/alignment centered")
    );

    let suggestions = app.get_suggestions_for("/compact mo se");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/compact mode semantic")
    );

    let suggestions = app.get_suggestions_for("/memory st");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/memory status")
    );

    let suggestions = app.get_suggestions_for("/improve st");
    assert!(
        suggestions.iter().any(|(cmd, _)| cmd == "/improve status"),
        "expected /improve status suggestion"
    );

    let suggestions = app.get_suggestions_for("/refactor st");
    assert!(
        suggestions.iter().any(|(cmd, _)| cmd == "/refactor status"),
        "expected /refactor status suggestion"
    );
}

#[test]
fn test_autocomplete_adds_space_for_nested_argument_commands() {
    let mut app = create_test_app();
    app.input = "/goals sh".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/goals show ");
}

#[test]
fn test_goals_show_suggestions_include_goal_ids() {
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

    let suggestions = app.get_suggestions_for("/goals show ");
    assert!(
        suggestions
            .iter()
            .any(|(cmd, _)| cmd == &format!("/goals show {}", goal.id))
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

fn configure_test_remote_models(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.3-codex".to_string());
    app.remote_available_entries = vec!["gpt-5.3-codex".to_string(), "gpt-5.2-codex".to_string()];
}

fn configure_test_remote_models_with_openai_recommendations(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("gpt-5.2".to_string());
    app.remote_available_entries = vec![
        "gpt-5.2".to_string(),
        "gpt-5.4".to_string(),
        "gpt-5.4-pro".to_string(),
        "gpt-5.3-codex-spark".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4-7".to_string(),
    ];
    app.remote_model_options = app
        .remote_available_entries
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

fn configure_test_remote_openrouter_provider_routes(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("anthropic/claude-sonnet-4".to_string());
    app.remote_available_entries = vec!["anthropic/claude-sonnet-4".to_string()];
    app.remote_model_options = vec![
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "auto".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "→ Fireworks".to_string(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "Fireworks".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
        crate::provider::ModelRoute {
            model: "anthropic/claude-sonnet-4".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        },
    ];
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
fn test_login_picker_preview_filter_parsing() {
    assert_eq!(
        App::login_picker_preview_filter("/login"),
        Some(String::new())
    );
    assert_eq!(
        App::login_picker_preview_filter("/login   zai"),
        Some("zai".to_string())
    );
    assert_eq!(App::login_picker_preview_filter("/loginx"), None);
    assert_eq!(App::login_picker_preview_filter("hello /login"), None);
}

#[test]
fn test_agents_command_opens_agent_picker() {
    let mut app = create_test_app();
    app.input = "/agents".to_string();

    app.submit_input();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("/agents should open the agent picker");
    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "Code review")
    );
    assert!(picker.entries.iter().any(|entry| matches!(
        entry.action,
        crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm)
    )));
}

#[test]
fn test_agents_command_suggestions_include_targets() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/agents re");
    assert!(suggestions.iter().any(|(cmd, _)| cmd == "/agents review"));
}

#[test]
fn test_agents_picker_uses_provider_default_when_inherited_model_is_unknown() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.open_agents_picker();

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("/agents should open the agent picker");
        let swarm_entry = picker
            .entries
            .iter()
            .find(|entry| {
                matches!(
                    entry.action,
                    crate::tui::PickerAction::AgentTarget(crate::tui::AgentModelTarget::Swarm)
                )
            })
            .expect("swarm entry should exist");

        assert_eq!(swarm_entry.options[0].provider, "provider default");
    });
}

#[test]
fn test_agent_model_picker_inherit_row_uses_provider_default_when_inherited_model_is_unknown() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Swarm);

        let picker = app
            .inline_interactive_state
            .as_ref()
            .expect("agent model picker should open");
        let inherit_entry = picker.entries.first().expect("inherit row should exist");

        assert_eq!(inherit_entry.name, "inherit (provider default)");
        assert!(matches!(
            inherit_entry.action,
            crate::tui::PickerAction::AgentModelChoice {
                target: crate::tui::AgentModelTarget::Swarm,
                clear_override: true,
            }
        ));
    });
}

#[test]
fn test_agents_review_picker_saves_config_override() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        configure_test_remote_models(&mut app);
        app.open_agent_model_picker(crate::tui::AgentModelTarget::Review);

        let selected = app
            .inline_interactive_state
            .as_ref()
            .and_then(|picker| {
                picker.filtered.iter().position(|&idx| {
                    matches!(
                        picker.entries[idx].action,
                        crate::tui::PickerAction::AgentModelChoice {
                            target: crate::tui::AgentModelTarget::Review,
                            clear_override: false,
                        }
                    )
                })
            })
            .expect("review picker should include at least one model option");
        app.inline_interactive_state.as_mut().unwrap().selected = selected;
        let selected_model_idx = app.inline_interactive_state.as_ref().unwrap().filtered[selected];
        app.inline_interactive_state.as_mut().unwrap().entries[selected_model_idx].options[0]
            .available = true;

        let expected = {
            let picker = app.inline_interactive_state.as_ref().unwrap();
            let entry = &picker.entries[picker.filtered[selected]];
            let base = if entry.effort.is_some() {
                entry
                    .name
                    .rsplit_once(" (")
                    .map(|(base, _)| base.to_string())
                    .unwrap_or_else(|| entry.name.clone())
            } else {
                entry.name.clone()
            };
            let route = &entry.options[entry.selected_option];
            if route.api_method == "copilot" {
                format!("copilot:{}", base)
            } else if route.api_method == "cursor" {
                format!("cursor:{}", base)
            } else if route.api_method == "openrouter" && route.provider != "auto" {
                if base.contains('/') {
                    format!("{}@{}", base, route.provider)
                } else {
                    format!("anthropic/{}@{}", base, route.provider)
                }
            } else {
                base
            }
        };

        app.handle_inline_interactive_key(KeyCode::Enter, KeyModifiers::NONE)
            .expect("save agent model override");

        let cfg = crate::config::Config::load();
        assert_eq!(cfg.autoreview.model.as_deref(), Some(expected.as_str()));
        assert!(app.inline_interactive_state.is_none());
    });
}

#[test]
fn test_model_command_suggestions_include_matching_models() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    let suggestions = app.get_suggestions_for("/model g52c");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/model gpt-5.2-codex")
    );
}

#[test]
fn test_model_command_trailing_space_shows_model_suggestions() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);

    let suggestions = app.get_suggestions_for("/model ");
    assert!(
        suggestions
            .iter()
            .any(|(cmd, _)| cmd == "/model gpt-5.3-codex")
    );
}

#[test]
fn test_model_command_provider_suggestions_include_openrouter_routes() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    let suggestions = app.get_suggestions_for("/model anthropic/claude-sonnet-4@");
    let commands: Vec<&str> = suggestions.iter().map(|(cmd, _)| cmd.as_str()).collect();

    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@auto"));
    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@Fireworks"));
    assert!(commands.contains(&"/model anthropic/claude-sonnet-4@OpenAI"));
}

#[test]
fn test_model_command_provider_suggestions_rank_matching_provider_prefix() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    let suggestions = app.get_suggestions_for("/model anthropic/claude-sonnet-4@fi");
    assert_eq!(
        suggestions.first().map(|(cmd, _)| cmd.as_str()),
        Some("/model anthropic/claude-sonnet-4@Fireworks")
    );
}

#[test]
fn test_login_command_suggestions_follow_provider_catalog() {
    let app = create_test_app();
    let suggestions = app.get_suggestions_for("/login ");

    for provider in crate::provider_catalog::tui_login_providers() {
        assert!(
            suggestions
                .iter()
                .any(|(cmd, detail)| cmd == &format!("/login {}", provider.id)
                    && detail == &provider.menu_detail),
            "missing /login suggestion for provider {}",
            provider.id
        );
    }
}

#[test]
fn test_model_autocomplete_completes_unique_match() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);
    app.input = "/model g52c".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/model gpt-5.2-codex");
}

#[test]
fn test_model_autocomplete_completes_unique_provider_match() {
    let mut app = create_test_app();
    configure_test_remote_openrouter_provider_routes(&mut app);

    app.input = "/model anthropic/claude-sonnet-4@fi".to_string();
    app.cursor_pos = app.input.len();

    assert!(app.autocomplete());
    assert_eq!(app.input(), "/model anthropic/claude-sonnet-4@Fireworks");
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
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.filter, "g52c");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.entries[i].name == "gpt-5.2-codex")
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
    assert!(app.inline_interactive_state.is_none());
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
        .inline_interactive_state
        .as_ref()
        .expect("model picker preview should be open");
    assert!(picker.preview);
    let initial_selected = picker.selected;

    // Down arrow should navigate in preview mode
    app.handle_key(KeyCode::Down, KeyModifiers::empty())
        .unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected + 1);

    // Up arrow should navigate back
    app.handle_key(KeyCode::Up, KeyModifiers::empty()).unwrap();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("picker should still be open");
    assert!(picker.preview, "should remain in preview mode");
    assert_eq!(picker.selected, initial_selected);

    // Input should be preserved
    assert_eq!(app.input(), "/model");
}

#[test]
fn test_open_model_picker_without_routes_shows_actionable_guidance() {
    let mut app = create_test_app();

    app.open_model_picker();

    assert!(app.inline_interactive_state.is_none());
    assert_eq!(app.status_notice(), Some("No models available".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
    assert!(last.content.contains("/model"));
}

#[test]
fn test_local_model_picker_selection_failure_keeps_picker_open_and_shows_next_steps() {
    let mut app = create_failing_model_switch_test_app();

    app.open_model_picker();
    assert!(app.inline_interactive_state.is_some());

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .expect("enter should be handled");

    assert!(
        app.inline_interactive_state.is_some(),
        "picker should remain open so the user can choose another model"
    );
    assert_eq!(app.status_notice(), Some("Model switch failed".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("credentials expired"));
    assert!(last.content.contains("/model"));
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("/account"));
}

#[test]
fn test_login_completed_surfaces_new_provider_models_in_local_model_picker() {
    let mut app = create_auth_refresh_test_app();

    app.handle_login_completed(crate::bus::LoginCompleted {
        provider: "copilot".to_string(),
        success: true,
        message: "Authenticated as **octocat** via GitHub Copilot.\n\nCopilot models are now available in `/model`."
            .to_string(),
    });

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let copilot_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-opus-4.6")
        .expect("copilot model should be shown after login");

    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "grok-code-fast-1"),
        "all newly available Copilot models should appear in /model"
    );
    assert!(copilot_entry.options.iter().any(|route| {
        route.provider == "Copilot" && route.api_method == "copilot" && route.available
    }));
}

#[test]
fn test_local_model_picker_surfaces_antigravity_models_from_multiprovider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let antigravity_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be shown after login");

    assert!(antigravity_entry.options.iter().any(|route| {
        route.provider == "Antigravity" && route.api_method == "cli" && route.available
    }));
}

#[test]
fn test_local_antigravity_model_picker_selection_preserves_antigravity_provider() {
    let mut app = create_antigravity_picker_test_app();
    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_idx = picker
        .entries
        .iter()
        .position(|entry| entry.name == "claude-sonnet-4-6")
        .expect("antigravity model should be in picker");
    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == model_idx)
        .expect("antigravity model should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(app.provider.name(), "Antigravity");
    assert_eq!(app.provider.model(), "claude-sonnet-4-6");
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_login_picker_preview_stays_open_and_updates_filter() {
    let mut app = create_test_app();

    for c in "/login za".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("login picker preview should be open");
    assert!(picker.preview);
    assert_eq!(picker.kind, crate::tui::PickerKind::Login);
    assert_eq!(picker.filter, "za");
    assert!(
        picker
            .filtered
            .iter()
            .any(|&i| picker.entries[i].name == "Z.AI")
    );
    assert_eq!(app.input(), "/login za");
}

#[test]
fn test_login_picker_preview_enter_starts_login_flow() {
    let mut app = create_test_app();

    for c in "/login zai".chars() {
        app.handle_key(KeyCode::Char(c), KeyModifiers::empty())
            .unwrap();
    }
    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert!(app.inline_interactive_state.is_none());
    match app.pending_login {
        Some(crate::tui::app::auth::PendingLogin::ApiKeyProfile {
            provider,
            openai_compatible_profile: Some(profile),
            ..
        }) => {
            assert_eq!(provider, "Z.AI");
            assert_eq!(profile.id, crate::provider_catalog::ZAI_PROFILE.id);
        }
        ref other => panic!("unexpected pending login state: {other:?}"),
    }
}

#[test]
fn test_subagent_model_command_sets_and_resets_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model gpt-5.4"
    ));
    assert_eq!(app.session.subagent_model.as_deref(), Some("gpt-5.4"));

    assert!(super::commands::handle_session_command(
        &mut app,
        "/subagent-model inherit"
    ));
    assert_eq!(app.session.subagent_model, None);
}

#[test]
fn test_autoreview_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview on"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(true));
    assert!(app.autoreview_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autoreview off"
    ));
    assert_eq!(app.session.autoreview_enabled, Some(false));
    assert!(!app.autoreview_enabled);
}

#[test]
fn test_autojudge_command_toggles_session_preference() {
    let mut app = create_test_app();

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge on"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(true));
    assert!(app.autojudge_enabled);

    assert!(super::commands::handle_session_command(
        &mut app,
        "/autojudge off"
    ));
    assert_eq!(app.session.autojudge_enabled, Some(false));
    assert!(!app.autojudge_enabled);
}

#[test]
fn test_poke_arms_auto_poke_until_todos_are_done() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Finish the remaining task".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        assert!(super::commands::handle_session_command(&mut app, "/poke"));

        assert!(app.auto_poke_incomplete_todos);
        assert!(app.pending_turn);
        assert!(
            app.display_messages()
                .iter()
                .any(|msg| msg.content.contains("Poking model with 1 incomplete todo"))
        );
    });
}

#[test]
fn test_finish_turn_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "in_progress".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        super::local::finish_turn(&mut app);

        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Please continue your work"));
    });
}

#[test]
fn test_finish_turn_does_not_duplicate_existing_poke_followup() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Keep going".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.queued_messages.push("existing poke".to_string());
        super::local::finish_turn(&mut app);

        assert_eq!(app.queued_messages(), &["existing poke"]);
    });
}

#[test]
fn test_review_prefers_openai_oauth_gpt_5_4_when_available() {
    with_temp_jcode_home(|| {
        let auth_path = crate::storage::jcode_dir()
            .expect("jcode dir")
            .join("openai-auth.json");
        std::fs::write(
            &auth_path,
            serde_json::json!({
                "openai_accounts": [
                    {
                        "label": "openai-1",
                        "access_token": "at_test",
                        "refresh_token": "rt_test",
                        "account_id": "acct_test"
                    }
                ],
                "active_openai_account": "openai-1"
            })
            .to_string(),
        )
        .expect("write auth file");

        assert_eq!(
            super::commands::preferred_one_shot_review_override(),
            Some(("gpt-5.4".to_string(), "openai".to_string()))
        );
    });
}

#[test]
fn test_pending_split_launch_shows_processing_status_in_ui() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now());

    assert!(app.is_processing());
    assert!(crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_some());
}

#[test]
fn test_expired_pending_split_launch_no_longer_shows_processing_status() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_split_started_at = Some(Instant::now() - Duration::from_millis(400));

    assert!(!app.is_processing());
    assert!(!crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Idle
    ));
    assert!(crate::tui::TuiState::elapsed(&app).is_none());
}

#[test]
fn test_pending_remote_dispatch_counts_as_processing_for_tui_state() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.pending_queued_dispatch = true;

    assert!(app.is_processing());
    assert!(crate::tui::TuiState::is_processing(&app));
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
}

#[test]
fn test_startup_message_restore_uses_hidden_system_queue() {
    with_temp_jcode_home(|| {
        let session_id = "startup-hidden-queue-test";
        super::App::save_startup_message_for_session(
            session_id,
            "internal startup prompt".to_string(),
        );

        let restored = super::App::restore_input_for_reload(session_id)
            .expect("startup message should restore");
        assert!(restored.queued_messages.is_empty());
        assert_eq!(
            restored.hidden_queued_system_messages,
            vec!["internal startup prompt".to_string()]
        );
    });
}

#[test]
fn test_review_and_judge_startup_prompts_are_analysis_only() {
    let prompts = [
        super::commands::build_autoreview_startup_message("session_parent"),
        super::commands::build_review_startup_message("session_parent"),
        super::commands::build_autojudge_startup_message("session_parent"),
        super::commands::build_judge_startup_message("session_parent"),
    ];

    for prompt in prompts {
        assert!(prompt.contains("analysis-only"));
        assert!(prompt.contains("Do not do the work yourself"));
        assert!(prompt.contains("Do not modify files or repo state"));
        assert!(prompt.contains("send exactly one DM"));
        assert!(prompt.contains("Do not continue implementation"));
    }
}

#[test]
fn test_autojudge_prompt_is_continue_or_stop_manager() {
    let prompt = super::commands::build_autojudge_startup_message("session_parent");

    assert!(prompt.contains("act like a strong completion manager/reviewer"));
    assert!(prompt.contains("tell it exactly what to do next"));
    assert!(prompt.contains("Default to `CONTINUE:` unless you are genuinely convinced"));
    assert!(prompt.contains("Start with either `CONTINUE:` or `STOP:`"));
    assert!(prompt.contains("Address the DM to the parent agent, not to the user"));
}

#[test]
fn test_judge_startup_prompts_describe_visible_mirror_context() {
    let prompts = [
        super::commands::build_autojudge_startup_message("session_parent"),
        super::commands::build_judge_startup_message("session_parent"),
    ];

    for prompt in prompts {
        assert!(prompt.contains("user-visible mirror of the parent conversation"));
        assert!(prompt.contains("shallow summaries of visible tool calls"));
        assert!(prompt.contains("omits deep tool-result details"));
    }
}

#[test]
fn test_prepare_review_spawned_session_uses_visible_transcript_for_judge_sessions() {
    with_temp_jcode_home(|| {
        for title in ["judge", "autojudge"] {
            let parent_id = format!("parent_{title}_visible_context");
            let child_id = format!("child_{title}_visible_context");
            let tool_id = format!("tool_{title}_visible_context");

            let mut parent = crate::session::Session::create_with_id(
                parent_id.clone(),
                None,
                Some("parent".to_string()),
            );
            parent.add_message(
                Role::User,
                vec![ContentBlock::Text {
                    text: "please review what happened".to_string(),
                    cache_control: None,
                }],
            );
            parent.add_message(
                Role::Assistant,
                vec![
                    ContentBlock::Text {
                        text: "I inspected the repo.".to_string(),
                        cache_control: None,
                    },
                    ContentBlock::ToolUse {
                        id: tool_id.clone(),
                        name: "bash".to_string(),
                        input: serde_json::json!({"command": "git diff --stat"}),
                    },
                ],
            );
            parent.add_message(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: tool_id.clone(),
                    content: "SECRET_TOOL_OUTPUT_SHOULD_NOT_APPEAR".to_string(),
                    is_error: None,
                }],
            );
            parent.add_message(
                Role::Assistant,
                vec![
                    ContentBlock::Reasoning {
                        text: "hidden reasoning should never leak".to_string(),
                    },
                    ContentBlock::Text {
                        text: "Final visible answer.".to_string(),
                        cache_control: None,
                    },
                ],
            );
            parent.save().expect("save parent session");

            let mut child = crate::session::Session::create_with_id(
                child_id.clone(),
                Some(parent_id.clone()),
                Some(title.to_string()),
            );
            child.replace_messages(parent.messages.clone());
            child.compaction = Some(crate::session::StoredCompactionState {
                summary_text: "stale compaction".to_string(),
                openai_encrypted_content: None,
                covers_up_to_turn: 1,
                original_turn_count: 1,
                compacted_count: 1,
            });
            child.save().expect("save child session");

            super::commands::prepare_review_spawned_session(
                &child_id,
                super::commands::build_judge_startup_message(&parent_id),
                None,
                None,
                Some(title.to_string()),
                Some(parent_id.clone()),
            );

            let prepared = crate::session::Session::load(&child_id).expect("reload child session");
            let transcript = prepared
                .messages
                .iter()
                .flat_map(|msg| msg.content.iter())
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n\n");

            assert!(transcript.contains("please review what happened"));
            assert!(transcript.contains("I inspected the repo."));
            assert!(transcript.contains("Final visible answer."));
            assert!(transcript.contains("Visible tool call"));
            assert!(transcript.contains("git diff --stat"));
            assert!(!transcript.contains("SECRET_TOOL_OUTPUT_SHOULD_NOT_APPEAR"));
            assert!(!transcript.contains("hidden reasoning should never leak"));
            assert_eq!(prepared.parent_id.as_deref(), Some(parent_id.as_str()));
            assert!(prepared.compaction.is_none());
        }
    });
}

#[test]
fn test_queue_autojudge_remote_targets_original_non_judge_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;

        let mut root = crate::session::Session::create(None, Some("task".to_string()));
        root.save().expect("save root session");

        let mut review =
            crate::session::Session::create(Some(root.id.clone()), Some("review".to_string()));
        review.save().expect("save review session");

        let mut judge =
            crate::session::Session::create(Some(review.id.clone()), Some("judge".to_string()));
        judge.save().expect("save judge session");

        app.session = judge.clone();
        app.remote_session_id = Some(judge.id.clone());
        app.autojudge_enabled = true;

        super::commands::queue_autojudge_remote(&mut app);

        assert_eq!(
            app.pending_split_parent_session_id.as_deref(),
            Some(root.id.as_str())
        );
        let startup = app
            .pending_split_startup_message
            .as_deref()
            .expect("autojudge startup message");
        assert!(startup.contains(root.id.as_str()));
        assert!(!startup.contains(review.id.as_str()));
        assert!(!startup.contains(judge.id.as_str()));
    });
}

#[test]
fn test_new_for_remote_restores_spawn_startup_hints_and_dispatch_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_spawn_child";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("spawn child".to_string()),
        );
        session.save().expect("save spawned child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_autojudge_startup_message("session_parent_123"),
        );

        let app = App::new_for_remote(Some(session_id.to_string()));

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert!(app.processing_started.is_some());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.status_notice(), Some("Autojudge starting".to_string()));
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        let startup_banner = app
            .display_messages()
            .last()
            .expect("spawned session should show startup banner");
        assert_eq!(startup_banner.role, "system");
        assert_eq!(startup_banner.title.as_deref(), Some("Autojudge"));
        assert!(startup_banner.content.contains("analysis-only"));
        assert!(
            startup_banner
                .content
                .contains("send exactly one DM back telling the parent either to `CONTINUE:`")
        );
        assert!(startup_banner.content.contains("user-visible mirror"));
        assert!(startup_banner.content.contains("session_parent_123"));
    });
}

#[test]
fn test_remote_startup_done_event_does_not_cancel_pending_judge_launch() {
    with_temp_jcode_home(|| {
        let session_id = "session_judge_startup_done_guard";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge child".to_string()),
        );
        session.save().expect("save judge child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_guard"),
        );

        let mut app = App::new_for_remote(Some(session_id.to_string()));
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert_eq!(app.current_message_id, None);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        app.handle_server_event(crate::protocol::ServerEvent::Done { id: 1 }, &mut remote);

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.current_message_id, None);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);
    });
}

#[test]
fn test_remote_startup_judge_hidden_prompt_dispatches_once_history_is_loaded() {
    with_temp_jcode_home(|| {
        let session_id = "session_judge_startup_dispatch";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge child".to_string()),
        );
        session.save().expect("save judge child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_dispatch"),
        );

        let mut app = App::new_for_remote(Some(session_id.to_string()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        remote.mark_history_loaded();

        assert!(app.pending_queued_dispatch);
        assert!(app.is_processing());
        assert_eq!(app.current_message_id, None);

        app.pending_queued_dispatch = false;
        rt.block_on(super::remote::process_remote_followups(
            &mut app,
            &mut remote,
        ));

        assert!(app.hidden_queued_system_messages.is_empty());
        assert!(app.is_processing());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert!(app.current_message_id.is_some());
    });
}

#[test]
fn test_new_for_remote_fresh_spawn_skips_local_transcript_restore() {
    with_temp_jcode_home(|| {
        let session_id = "session_spawn_fresh_skip";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("spawn fresh".to_string()),
        );
        session.model = Some("gpt-5.4".to_string());
        session.append_stored_message(crate::session::StoredMessage {
            id: "msg_spawn_fresh_skip".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![crate::message::ContentBlock::Text {
                text: "persisted transcript should not be restored locally".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save spawned child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_autojudge_startup_message("session_parent_123"),
        );

        let app = App::new_for_remote_with_options(Some(session_id.to_string()), true);

        assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.hidden_queued_system_messages.len(), 1);
        assert_eq!(app.display_messages().len(), 1);
        let startup_banner = app.display_messages().last().expect("startup banner");
        assert_eq!(startup_banner.role, "system");
        assert_eq!(startup_banner.title.as_deref(), Some("Autojudge"));
        assert!(
            !startup_banner
                .content
                .contains("persisted transcript should not be restored locally")
        );
    });
}

#[test]
fn test_new_for_remote_restores_display_history_without_retaining_session_transcript() {
    with_temp_jcode_home(|| {
        let session_id = "session_remote_restore_lightweight";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("remote resume".to_string()),
        );
        session.model = Some("gpt-5.4".to_string());
        session.append_stored_message(crate::session::StoredMessage {
            id: "msg_remote_restore_1".to_string(),
            role: crate::message::Role::Assistant,
            content: vec![crate::message::ContentBlock::Text {
                text: "persisted transcript should render once".to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save remote restore session");

        let app = App::new_for_remote_with_options(Some(session_id.to_string()), false);

        assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
        assert_eq!(app.display_messages().len(), 1);
        assert_eq!(
            app.display_messages()[0].content,
            "persisted transcript should render once"
        );
        assert!(app.session.messages.is_empty());
        assert!(app.session.compaction.is_none());
    });
}

#[test]
fn test_restore_session_restores_local_judge_processing_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_local_judge_child";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("judge".to_string()),
        );
        session.save().expect("save child session");

        super::App::save_startup_message_for_session(
            session_id,
            super::commands::build_judge_startup_message("session_parent_local"),
        );

        let mut app = create_test_app();
        app.restore_session(session_id);

        assert!(app.is_processing());
        assert!(app.pending_turn);
        assert!(app.processing_started.is_some());
        assert!(matches!(
            crate::tui::TuiState::status(&app),
            ProcessingStatus::Sending
        ));
        assert_eq!(app.status_notice(), Some("Judge starting".to_string()));
        assert_eq!(app.hidden_queued_system_messages.len(), 1);

        let startup_banner = app
            .display_messages()
            .iter()
            .find(|msg| msg.title.as_deref() == Some("Judge"))
            .expect("judge restore should show startup banner");
        assert!(startup_banner.content.contains("session_parent_local"));
        assert!(startup_banner.content.contains("user-visible mirror"));
    });
}

#[test]
fn test_subagent_command_suggestions_include_manual_launch_and_model_policy() {
    let app = create_test_app();

    let subagent = app.get_suggestions_for("/subagent");
    assert!(subagent.iter().any(|(cmd, _)| cmd == "/subagent "));

    let model = app.get_suggestions_for("/subagent-model ");
    assert!(
        model
            .iter()
            .any(|(cmd, _)| cmd == "/subagent-model inherit")
    );

    let review = app.get_suggestions_for("/review");
    assert!(review.iter().any(|(cmd, _)| cmd == "/review"));

    let judge = app.get_suggestions_for("/judge");
    assert!(judge.iter().any(|(cmd, _)| cmd == "/judge"));

    let autojudge = app.get_suggestions_for("/autojudge");
    assert!(autojudge.iter().any(|(cmd, _)| cmd == "/autojudge status"));
}

fn configure_test_remote_models_with_copilot(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_model = Some("claude-sonnet-4".to_string());
    app.remote_available_entries = vec![
        "claude-sonnet-4-6".to_string(),
        "gpt-5.3-codex".to_string(),
        "claude-opus-4.6".to_string(),
        "gemini-3-pro-preview".to_string(),
        "grok-code-fast-1".to_string(),
    ];
}

fn configure_test_remote_models_with_cursor(app: &mut App) {
    app.is_remote = true;
    app.remote_provider_name = Some("cursor".to_string());
    app.remote_provider_model = Some("composer-1.5".to_string());
    app.remote_available_entries = vec![
        "composer-2-fast".to_string(),
        "composer-2".to_string(),
        "composer-1.5".to_string(),
    ];
    app.remote_model_options = app
        .remote_available_entries
        .iter()
        .cloned()
        .map(|model| crate::provider::ModelRoute {
            model,
            provider: "Cursor".to_string(),
            api_method: "cursor".to_string(),
            available: true,
            detail: String::new(),
            cheapness: None,
        })
        .collect();
}

#[test]
fn test_model_picker_includes_copilot_models_in_remote_mode() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.entries.iter().map(|m| m.name.as_str()).collect();

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
fn test_available_models_updated_event_surfaces_authed_provider_in_remote_model_picker() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::AvailableModelsUpdated {
            available_models: vec![
                "claude-opus-4.6".to_string(),
                "grok-code-fast-1".to_string(),
            ],
            available_model_routes: vec![
                crate::provider::ModelRoute {
                    model: "claude-opus-4.6".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
                crate::provider::ModelRoute {
                    model: "grok-code-fast-1".to_string(),
                    provider: "Copilot".to_string(),
                    api_method: "copilot".to_string(),
                    available: true,
                    detail: String::new(),
                    cheapness: None,
                },
            ],
        },
        &mut remote,
    );

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let copilot_entry = picker
        .entries
        .iter()
        .find(|entry| entry.name == "claude-opus-4.6")
        .expect("copilot model should be shown after AvailableModelsUpdated");

    assert!(
        picker
            .entries
            .iter()
            .any(|entry| entry.name == "grok-code-fast-1"),
        "all auth-updated remote models should appear in /model"
    );
    assert!(copilot_entry.options.iter().any(|route| {
        route.provider == "Copilot" && route.api_method == "copilot" && route.available
    }));
}

#[test]
fn test_remote_model_switch_failure_shows_actionable_guidance() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.handle_server_event(
        crate::protocol::ServerEvent::ModelChanged {
            id: 7,
            model: "claude-opus-4.6".to_string(),
            provider_name: Some("Copilot".to_string()),
            error: Some("credentials expired".to_string()),
        },
        &mut remote,
    );

    assert_eq!(app.status_notice(), Some("Model switch failed".to_string()));

    let last = app.display_messages.last().expect("display message");
    assert_eq!(last.role, "error");
    assert!(last.content.contains("credentials expired"));
    assert!(last.content.contains("/model"));
    assert!(last.content.contains("/login"));
    assert!(last.content.contains("reconnect"));
}

#[test]
fn test_model_picker_remote_falls_back_to_current_model_when_catalog_empty() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.remote_provider_name = Some("openrouter".to_string());
    app.remote_provider_model = Some("anthropic/claude-sonnet-4".to_string());
    app.remote_available_entries.clear();
    app.remote_model_options.clear();

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should open with current-model fallback");

    assert_eq!(picker.entries.len(), 1);
    assert_eq!(picker.entries[0].name, "anthropic/claude-sonnet-4");
    assert_eq!(picker.entries[0].options.len(), 1);
    assert_eq!(picker.entries[0].options[0].provider, "openrouter");
    assert_eq!(picker.entries[0].options[0].api_method, "current");
    assert!(picker.entries[0].options[0].available);
}

#[test]
fn test_handle_server_event_available_models_updated_replaces_remote_model_catalog() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_remote = true;
    app.remote_available_entries = vec!["old-model".to_string()];
    app.remote_model_options = vec![crate::provider::ModelRoute {
        model: "old-model".to_string(),
        provider: "OldProvider".to_string(),
        api_method: "old-api".to_string(),
        available: false,
        detail: "old".to_string(),
        cheapness: None,
    }];

    app.handle_server_event(
        crate::protocol::ServerEvent::AvailableModelsUpdated {
            available_models: vec!["new-model".to_string(), "second-model".to_string()],
            available_model_routes: vec![crate::provider::ModelRoute {
                model: "new-model".to_string(),
                provider: "OpenAI".to_string(),
                api_method: "openai-oauth".to_string(),
                available: true,
                detail: String::new(),
                cheapness: None,
            }],
        },
        &mut remote,
    );

    assert_eq!(
        app.remote_available_entries,
        vec!["new-model".to_string(), "second-model".to_string()]
    );
    assert_eq!(app.remote_model_options.len(), 1);
    assert_eq!(app.remote_model_options[0].model, "new-model");
    assert_eq!(app.remote_model_options[0].provider, "OpenAI");
    assert!(app.remote_model_options[0].available);
}

#[test]
fn test_model_picker_copilot_models_have_copilot_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    // grok-code-fast-1 is NOT in ALL_CLAUDE_MODELS or ALL_OPENAI_MODELS,
    // so it should get a copilot route
    let grok_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "grok-code-fast-1")
        .expect("grok-code-fast-1 should be in picker");

    assert!(
        grok_entry.options.iter().any(|r| r.api_method == "copilot"),
        "grok-code-fast-1 should have a copilot route, got: {:?}",
        grok_entry.options
    );
}

#[test]
fn test_model_picker_preserves_recommendation_priority_order() {
    let mut app = create_test_app();
    configure_test_remote_models_with_openai_recommendations(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let model_names: Vec<&str> = picker.entries.iter().map(|m| m.name.as_str()).collect();

    assert_eq!(model_names.first().copied(), Some("gpt-5.2"));

    let gpt54 = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.4")
        .expect("gpt-5.4 should be present");
    let gpt54_pro = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.4-pro")
        .expect("gpt-5.4-pro should be present");
    let claude_opus = picker
        .entries
        .iter()
        .position(|model| model.name == "claude-opus-4-7")
        .expect("claude-opus-4-7 should be present");
    let spark = picker
        .entries
        .iter()
        .position(|model| model.name == "gpt-5.3-codex-spark")
        .expect("gpt-5.3-codex-spark should be present");
    let codex = picker
        .entries
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
        "gpt-5.4-pro should rank ahead of claude-opus-4-7, got {:?}",
        model_names
    );
    assert!(
        claude_opus < spark,
        "claude-opus-4-7 should rank ahead of non-recommended gpt-5.3-codex-spark, got {:?}",
        model_names
    );
    assert!(
        !picker.entries[spark].recommended,
        "gpt-5.3-codex-spark should not be recommended"
    );
    assert!(
        !picker.entries[codex].recommended,
        "gpt-5.3-codex should not be recommended"
    );
}

#[test]
fn test_model_picker_copilot_selection_prefixes_model() {
    let mut app = create_test_app();
    configure_test_remote_models_with_copilot(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    // Find grok-code-fast-1 (which should only be a copilot route)
    let grok_idx = picker
        .entries
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
    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

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
    assert!(app.inline_interactive_state.is_none());
}

#[test]
fn test_model_picker_cursor_models_have_cursor_route() {
    let mut app = create_test_app();
    configure_test_remote_models_with_cursor(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let composer_entry = picker
        .entries
        .iter()
        .find(|m| m.name == "composer-2-fast")
        .expect("composer-2-fast should be in picker");

    assert!(
        composer_entry
            .options
            .iter()
            .any(|r| r.api_method == "cursor"),
        "composer-2-fast should have a cursor route, got: {:?}",
        composer_entry.options
    );
}

#[test]
fn test_model_picker_cursor_selection_prefixes_model() {
    let mut app = create_test_app();
    configure_test_remote_models_with_cursor(&mut app);

    app.open_model_picker();

    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should be open");

    let composer_idx = picker
        .entries
        .iter()
        .position(|m| m.name == "composer-2-fast")
        .expect("composer-2-fast should be in picker");

    let filtered_pos = picker
        .filtered
        .iter()
        .position(|&i| i == composer_idx)
        .expect("composer-2-fast should be in filtered list");

    app.inline_interactive_state.as_mut().unwrap().selected = filtered_pos;

    app.handle_key(KeyCode::Enter, KeyModifiers::empty())
        .unwrap();

    assert_eq!(
        app.pending_model_switch.as_deref(),
        Some("cursor:composer-2-fast")
    );
    assert!(app.inline_interactive_state.is_none());
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
fn test_handle_key_ctrl_backspace_csi_u_char_fallback_deletes_word() {
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");

    app.handle_key(KeyCode::Char('\u{8}'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
}

#[test]
fn test_handle_key_ctrl_h_does_not_insert_text() {
    let mut app = create_test_app();
    app.set_input_for_test("hello");

    app.handle_key(KeyCode::Char('h'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), "hello".len());
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
    assert!(app.session_save_pending);
    assert!(matches!(app.status(), ProcessingStatus::Sending));
    assert!(app.elapsed().is_some());

    // Input should be cleared
    assert!(app.input().is_empty());
}

#[test]
fn test_submit_input_commits_pending_streaming_assistant_text_before_user_message() {
    let mut app = create_test_app();
    app.display_messages.push(DisplayMessage::tool(
        "file contents",
        crate::message::ToolCall {
            id: "tool_read".to_string(),
            name: "read".to_string(),
            input: serde_json::json!({"file_path": "src/main.rs"}),
            intent: None,
        },
    ));
    app.bump_display_messages_version();
    app.streaming_text = "Here is the final paragraph".to_string();
    assert_eq!(app.stream_buffer.push(" that was still buffered."), None);

    app.input = "follow up".to_string();
    app.cursor_pos = app.input.len();
    app.submit_input();

    assert_eq!(app.display_messages().len(), 3);
    assert_eq!(app.display_messages()[0].role, "tool");
    assert_eq!(app.display_messages()[1].role, "assistant");
    assert_eq!(
        app.display_messages()[1].content,
        "Here is the final paragraph that was still buffered."
    );
    assert_eq!(app.display_messages()[2].role, "user");
    assert_eq!(app.display_messages()[2].content, "follow up");
    assert!(app.streaming_text().is_empty());
    assert!(app.stream_buffer.is_empty());
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
fn test_ctrl_a_keeps_home_behavior_when_input_present() {
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();

    app.handle_key(KeyCode::Char('a'), KeyModifiers::CONTROL)
        .unwrap();

    assert_eq!(app.input(), "hello world");
    assert_eq!(app.cursor_pos(), 0);
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
fn test_handle_background_task_completed_renders_markdown_preview() {
    let mut app = create_test_app();
    let event = BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
        task_id: "bg123".to_string(),
        tool_name: "bash".to_string(),
        session_id: app.session.id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "[stderr] one\n[stdout] two\n".to_string(),
        output_file: std::env::temp_dir().join("bg123.output"),
        duration_secs: 7.1,
        notify: true,
        wake: false,
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    let rendered = app
        .display_messages()
        .last()
        .expect("background task message");
    assert_eq!(rendered.role, "background_task");
    assert!(
        rendered
            .content
            .contains("**Background task** `bg123` · `bash` · ✓ completed · 7.1s · exit 0")
    );
    assert!(rendered.content.contains("```text"));
    assert!(rendered.content.contains("[stderr] one"));
    assert!(
        rendered
            .content
            .contains("_Full output:_ `bg action=\"output\" task_id=\"bg123\"`")
    );
    assert_eq!(
        app.status_notice(),
        Some("Background task completed · bash".to_string())
    );
}

#[test]
fn test_handle_background_task_completed_with_wake_starts_pending_turn() {
    let mut app = create_test_app();
    let event = BusEvent::BackgroundTaskCompleted(BackgroundTaskCompleted {
        task_id: "bgwake".to_string(),
        tool_name: "selfdev-build".to_string(),
        session_id: app.session.id.clone(),
        status: BackgroundTaskStatus::Completed,
        exit_code: Some(0),
        output_preview: "done\n".to_string(),
        output_file: std::env::temp_dir().join("bgwake.output"),
        duration_secs: 1.2,
        notify: true,
        wake: true,
    });

    super::local::handle_bus_event(&mut app, Ok(event));

    assert!(app.pending_turn);
    assert!(app.is_processing());
    assert!(matches!(
        crate::tui::TuiState::status(&app),
        ProcessingStatus::Sending
    ));
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
fn test_build_turn_footer_uses_compact_duration_labels() {
    let app = create_test_app();

    assert_eq!(
        app.build_turn_footer(Some(316.1)),
        Some("5m 16s".to_string())
    );
    assert_eq!(app.build_turn_footer(Some(9.2)), Some("9.2s".to_string()));
}

#[test]
fn test_build_turn_footer_combines_compact_duration_with_streaming_stats() {
    let mut app = create_test_app();
    app.streaming_input_tokens = 210_000;
    app.streaming_output_tokens = 440;
    app.streaming_tps_collect_output = true;
    app.streaming_total_output_tokens = 440;
    app.streaming_tps_observed_output_tokens = 440;
    app.streaming_tps_observed_elapsed = Duration::from_secs(220);

    let footer = app
        .build_turn_footer(Some(316.1))
        .expect("footer with stats");

    assert!(
        footer.starts_with("5m 16s · "),
        "unexpected footer: {footer}"
    );
    assert!(footer.contains(" tps"), "unexpected footer: {footer}");
    assert!(
        footer.ends_with("↑210k ↓440"),
        "unexpected footer: {footer}"
    );
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

    // Local restore keeps provider messages lazy until the next active turn.
    assert_eq!(app.messages.len(), 0);
    assert_eq!(
        app.session.debug_memory_profile()["provider_messages_cache"]["count"],
        0
    );

    // Provider session ID should be cleared (Claude sessions don't persist across restarts)
    assert!(app.provider_session_id.is_none());

    // Clean up
    let _ = std::fs::remove_file(crate::session::session_path(&session_id).unwrap());
}

#[test]
fn test_restore_session_with_selfdev_reload_tool_result_queues_continuation() {
    use crate::session::Session;

    let mut app = create_test_app();

    let mut session = Session::create(None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: "tool_selfdev_reload".to_string(),
            content: "Reload initiated. Process restarting...".to_string(),
            is_error: Some(false),
        }],
    );
    let session_id = session.id.clone();
    session.save().unwrap();

    app.restore_session(&session_id);

    assert!(
        app.hidden_queued_system_messages
            .iter()
            .any(|message| message.contains("Continue exactly where you left off"))
    );
    assert!(app.pending_turn);
    assert!(matches!(app.status, ProcessingStatus::Sending));

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
fn test_background_update_ready_reloads_immediately_when_idle() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    app.handle_session_update_status(SessionUpdateStatus::ReadyToReload {
        session_id: session_id.clone(),
        action: ClientMaintenanceAction::Update,
        version: "v1.2.3".to_string(),
    });

    assert_eq!(app.reload_requested.as_deref(), Some(session_id.as_str()));
    assert!(app.should_quit);
}

#[test]
fn test_background_update_ready_waits_for_turn_to_finish() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.is_processing = true;

    app.handle_session_update_status(SessionUpdateStatus::ReadyToReload {
        session_id: session_id.clone(),
        action: ClientMaintenanceAction::Update,
        version: "v1.2.3".to_string(),
    });

    assert!(app.reload_requested.is_none());
    assert_eq!(
        app.pending_background_client_reload
            .as_ref()
            .map(|(id, action)| (id.as_str(), *action)),
        Some((session_id.as_str(), ClientMaintenanceAction::Update))
    );
    assert!(!app.should_quit);

    app.is_processing = false;
    crate::tui::app::local::handle_tick(&mut app);

    assert_eq!(app.reload_requested.as_deref(), Some(session_id.as_str()));
    assert!(app.should_quit);
}

#[test]
fn test_background_rebuild_status_uses_compact_rebuild_card() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();

    app.handle_session_update_status(SessionUpdateStatus::Status {
        session_id,
        action: ClientMaintenanceAction::Rebuild,
        message: "Building release binary in the background...".to_string(),
    });

    let message = app
        .display_messages()
        .last()
        .expect("expected rebuild display message");
    assert_eq!(message.title.as_deref(), Some("Rebuild"));
    assert!(
        message
            .content
            .contains("**Status:** Building release binary in the background...")
    );
    assert!(message.content.contains("**Pipeline:**"));
}

#[test]
fn test_selfdev_command_spawns_session_in_test_mode() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    let prev_test = std::env::var_os("JCODE_TEST_SESSION");
    crate::env::set_var("JCODE_HOME", temp_home.path());
    crate::env::set_var("JCODE_TEST_SESSION", "1");

    let repo = create_jcode_repo_fixture();
    let mut app = create_test_app();
    app.session.working_dir = Some(repo.path().display().to_string());

    app.input = "/selfdev fix the markdown renderer".to_string();
    app.submit_input();

    let last = app.display_messages().last().expect("selfdev message");
    assert!(last.content.contains("Created self-dev session"));
    assert!(
        last.content
            .contains("Prompt captured but not delivered in test mode")
    );
    assert_eq!(app.status_notice(), Some("Self-dev".to_string()));

    let sessions_dir = crate::storage::jcode_dir().unwrap().join("sessions");
    let entries: Vec<_> = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .flatten()
        .collect();
    assert!(
        !entries.is_empty(),
        "expected spawned self-dev session file"
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
    if let Some(prev_test) = prev_test {
        crate::env::set_var("JCODE_TEST_SESSION", prev_test);
    } else {
        crate::env::remove_var("JCODE_TEST_SESSION");
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
    assert_eq!(restored.input, "draft");
    assert_eq!(restored.cursor, 3);
    assert_eq!(restored.queued_messages, vec!["queued one", "queued two"]);
    assert_eq!(
        restored.hidden_queued_system_messages,
        vec!["continue silently"]
    );

    assert!(App::restore_input_for_reload(&session_id).is_none());
}

#[test]
fn test_save_and_restore_startup_submission_preserves_pending_images() {
    with_temp_jcode_home(|| {
        let session_id = "session_startup_prompt";
        App::save_startup_submission_for_session(
            session_id,
            "describe this".to_string(),
            vec![("image/png".to_string(), "abc123".to_string())],
        );

        let restored =
            App::restore_input_for_reload(session_id).expect("startup submission should restore");
        assert_eq!(restored.input, "describe this");
        assert!(restored.submit_on_restore);
        assert_eq!(restored.pending_images.len(), 1);
        assert_eq!(restored.pending_images[0].0, "image/png");
        assert_eq!(restored.pending_images[0].1, "abc123");
    });
}

#[test]
fn test_save_and_restore_reload_state_preserves_interleave_and_pending_retry() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-pending-{}", std::process::id());

    app.input = "draft".to_string();
    app.cursor_pos = 5;
    app.interleave_message = Some("urgent now".to_string());
    app.pending_soft_interrupts = vec![
        "already sent one".to_string(),
        "already sent two".to_string(),
    ];
    app.pending_soft_interrupt_requests = vec![(17, "already sent two".to_string())];
    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "retry me".to_string(),
        images: vec![("image/png".to_string(), "abc123".to_string())],
        is_system: true,
        system_reminder: Some("continue silently".to_string()),
        auto_retry: true,
        retry_attempts: 2,
        retry_at: None,
    });
    app.rate_limit_reset = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.interleave_message.as_deref(), Some("urgent now"));
    assert_eq!(
        restored.pending_soft_interrupts,
        vec!["already sent one", "already sent two"]
    );
    assert_eq!(
        restored.pending_soft_interrupt_resend,
        Some(vec!["already sent two".to_string()])
    );

    let pending = restored
        .rate_limit_pending_message
        .expect("pending retry should restore");
    assert_eq!(pending.content, "retry me");
    assert_eq!(
        pending.images,
        vec![("image/png".to_string(), "abc123".to_string())]
    );
    assert!(pending.is_system);
    assert_eq!(
        pending.system_reminder.as_deref(),
        Some("continue silently")
    );
    assert!(pending.auto_retry);
    assert_eq!(pending.retry_attempts, 2);
    assert!(pending.retry_at.is_some());
    assert!(restored.rate_limit_reset.is_some());
}

#[test]
fn test_save_and_restore_reload_state_promotes_inflight_prompt_to_startup_submission() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-inflight-prompt-{}", std::process::id());

    app.rate_limit_pending_message = Some(PendingRemoteMessage {
        content: "finish the refactor".to_string(),
        images: vec![("image/png".to_string(), "abc123".to_string())],
        is_system: false,
        system_reminder: None,
        auto_retry: false,
        retry_attempts: 0,
        retry_at: None,
    });
    app.rate_limit_reset = Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert_eq!(restored.input, "finish the refactor");
    assert_eq!(restored.cursor, "finish the refactor".len());
    assert!(
        restored.submit_on_restore,
        "in-flight prompt should resume automatically"
    );
    assert_eq!(restored.pending_images.len(), 1);
    assert!(
        restored.rate_limit_pending_message.is_none(),
        "promoted startup submission should not linger as a passive pending retry"
    );
}

#[test]
fn test_save_and_restore_reload_state_preserves_observe_mode() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-observe-{}", std::process::id());

    app.set_observe_mode_enabled(true, true);
    app.observe_page_markdown = "# Observe\n\nPersist me through reload.".to_string();
    app.observe_page_updated_at_ms = 42;
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert!(restored.observe_mode_enabled);
    assert_eq!(
        restored.observe_page_markdown,
        "# Observe\n\nPersist me through reload."
    );
    assert_eq!(restored.observe_page_updated_at_ms, 42);
}

#[test]
fn test_save_and_restore_reload_state_preserves_split_view_mode() {
    let mut app = create_test_app();
    let session_id = format!("test-reload-splitview-{}", std::process::id());

    app.set_split_view_enabled(true, true);
    app.save_input_for_reload(&session_id);

    let restored = App::restore_input_for_reload(&session_id).expect("reload state should exist");
    assert!(restored.split_view_enabled);
}

#[test]
fn test_new_for_remote_restores_observe_mode_from_reload_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-observe-{}", std::process::id());

    app.set_observe_mode_enabled(true, true);
    app.observe_page_markdown = "# Observe\n\nRestored after reload.".to_string();
    app.observe_page_updated_at_ms = 99;
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.observe_mode_enabled());
    let page = restored
        .side_panel()
        .focused_page()
        .expect("observe page should be focused");
    assert_eq!(page.id, "observe");
    assert!(page.content.contains("Restored after reload."));
}

#[test]
fn test_new_for_remote_restores_split_view_from_reload_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-splitview-{}", std::process::id());

    app.set_split_view_enabled(true, true);
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.split_view_enabled());
    let page = restored
        .side_panel()
        .focused_page()
        .expect("split view page should be focused");
    assert_eq!(page.id, "split_view");
    assert!(page.content.contains("Split View"));
}

#[test]
fn test_restore_reload_state_supports_legacy_input_format() {
    let session_id = format!("test-reload-legacy-{}", std::process::id());
    let jcode_dir = crate::storage::jcode_dir().unwrap();
    let path = jcode_dir.join(format!("client-input-{}", session_id));
    std::fs::write(&path, "2\nhello").unwrap();

    let restored =
        App::restore_input_for_reload(&session_id).expect("legacy reload state should restore");
    assert_eq!(restored.input, "hello");
    assert_eq!(restored.cursor, 2);
    assert!(restored.queued_messages.is_empty());
}

#[test]
fn test_new_for_remote_requeues_restored_pending_soft_interrupts() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-restore-{}", std::process::id());

    app.interleave_message = Some("local interleave".to_string());
    app.pending_soft_interrupts = vec!["sent one".to_string(), "sent two".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(101, "sent one".to_string()), (102, "sent two".to_string())];
    app.queued_messages.push("queued later".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(
        restored.queued_messages(),
        &["local interleave", "sent one", "sent two", "queued later"]
    );
}

#[test]
fn test_new_for_remote_restored_interleave_triggers_dispatch_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-interleave-dispatch-{}", std::process::id());

    app.interleave_message = Some("interrupt after reload".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(restored.queued_messages(), &["interrupt after reload"]);
    assert!(restored.pending_queued_dispatch);
    assert!(restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Sending));
}

#[test]
fn test_new_for_remote_restored_soft_interrupt_resend_triggers_dispatch_state() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-soft-interrupt-dispatch-{}", std::process::id());

    app.pending_soft_interrupts = vec!["sent interrupt".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "sent interrupt".to_string())];
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert!(restored.interleave_message.is_none());
    assert_eq!(restored.queued_messages(), &["sent interrupt"]);
    assert!(restored.pending_queued_dispatch);
    assert!(restored.is_processing);
    assert!(matches!(restored.status, ProcessingStatus::Sending));
}

#[test]
fn test_new_for_remote_does_not_requeue_acked_pending_soft_interrupts() {
    let mut app = create_test_app();
    let session_id = format!("test-remote-acked-{}", std::process::id());

    app.interleave_message = Some("local interleave".to_string());
    app.pending_soft_interrupts = vec!["already queued on server".to_string()];
    app.queued_messages.push("queued later".to_string());
    app.save_input_for_reload(&session_id);

    let restored = App::new_for_remote(Some(session_id));
    assert_eq!(
        restored.interleave_message.as_deref(),
        Some("local interleave")
    );
    assert_eq!(restored.queued_messages(), &["queued later"]);
}

#[test]
fn test_initial_history_bootstrap_preserves_restored_interleave_state() {
    with_temp_jcode_home(|| {
        let session_id = "session_reload_restore_interleave";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("reload restore".to_string()),
        );
        session.save().expect("save session for reload restore");

        let mut app = create_test_app();
        app.interleave_message = Some("interrupt after reload".to_string());
        app.pending_soft_interrupts = vec!["already sent interrupt".to_string()];
        app.pending_soft_interrupt_requests = vec![(55, "already sent interrupt".to_string())];
        app.queued_messages.push("queued followup".to_string());
        app.save_input_for_reload(session_id);

        let mut restored = App::new_for_remote(Some(session_id.to_string()));
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        restored.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: session_id.to_string(),
                messages: vec![],
                images: vec![],
                provider_name: Some("claude".to_string()),
                provider_model: Some("claude-sonnet-4-20250514".to_string()),
                subagent_model: None,
                autoreview_enabled: None,
                autojudge_enabled: None,
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
                status_detail: None,
                upstream_provider: None,
                reasoning_effort: None,
                service_tier: None,
                compaction_mode: crate::config::CompactionMode::Reactive,
                activity: None,
                side_panel: crate::side_panel::SidePanelSnapshot::default(),
            },
            &mut remote,
        );

        assert!(restored.interleave_message.is_none());
        assert_eq!(
            restored.queued_messages(),
            &[
                "interrupt after reload",
                "already sent interrupt",
                "queued followup"
            ]
        );
        assert!(
            restored.pending_soft_interrupts.is_empty(),
            "restored pending interrupts should remain represented by interleave + queue state"
        );
    });
}

#[test]
fn test_initial_history_bootstrap_skips_resubmit_when_prompt_already_in_history() {
    with_temp_jcode_home(|| {
        let session_id = "session_reload_prompt_already_in_history";
        let mut session = crate::session::Session::create_with_id(
            session_id.to_string(),
            None,
            Some("reload prompt already in history".to_string()),
        );
        session.save().expect("save session for reload restore");

        let mut app = create_test_app();
        app.rate_limit_pending_message = Some(PendingRemoteMessage {
            content: "continue implementing the fix".to_string(),
            images: vec![],
            is_system: false,
            system_reminder: None,
            auto_retry: false,
            retry_attempts: 0,
            retry_at: None,
        });
        app.save_input_for_reload(session_id);

        let mut restored = App::new_for_remote(Some(session_id.to_string()));
        assert!(restored.submit_input_on_startup);
        assert_eq!(restored.input, "continue implementing the fix");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        restored.handle_server_event(
            crate::protocol::ServerEvent::History {
                id: 1,
                session_id: session_id.to_string(),
                messages: vec![crate::protocol::HistoryMessage {
                    role: "user".to_string(),
                    content: "continue implementing the fix".to_string(),
                    tool_calls: None,
                    tool_data: None,
                }],
                images: vec![],
                provider_name: Some("claude".to_string()),
                provider_model: Some("claude-sonnet-4-20250514".to_string()),
                subagent_model: None,
                autoreview_enabled: None,
                autojudge_enabled: None,
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
                status_detail: None,
                upstream_provider: None,
                reasoning_effort: None,
                service_tier: None,
                compaction_mode: crate::config::CompactionMode::Reactive,
                activity: None,
                side_panel: crate::side_panel::SidePanelSnapshot::default(),
            },
            &mut remote,
        );

        assert!(!restored.submit_input_on_startup);
        assert!(restored.input.is_empty());
        assert!(
            restored
                .display_messages()
                .iter()
                .any(|message| message.content == "Reload complete — continuing."),
            "server interruption recovery should continue using the restored server-side prompt"
        );
    });
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
fn test_handle_server_event_updates_status_detail() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::StatusDetail {
            detail: "reusing websocket".to_string(),
        },
        &mut remote,
    );

    assert_eq!(app.status_detail.as_deref(), Some("reusing websocket"));
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
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id.clone());

    crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_123".to_string(),
            session_id: Some(session_id),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert_eq!(app.input, "draft dictated text");
    assert_eq!(app.status_notice(), Some("Transcript appended".to_string()));
}

#[test]
fn test_local_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let session_id = app.session.id.clone();
    app.input = "draft".to_string();
    app.cursor_pos = app.input.len();
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some(session_id);

    let handled = crate::tui::app::local::handle_bus_event(
        &mut app,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    );

    assert!(!handled);
    assert_eq!(app.input, "draft");
    assert!(app.dictation_in_flight);
}

#[test]
fn test_remote_bus_dictation_completion_ignores_other_session() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut remote = rt.block_on(async { crate::tui::backend::RemoteConnection::dummy() });
    app.is_remote = true;
    app.remote_session_id = Some("session_remote".to_string());
    app.dictation_in_flight = true;
    app.dictation_request_id = Some("dictation_123".to_string());
    app.dictation_target_session_id = Some("session_remote".to_string());

    rt.block_on(crate::tui::app::remote::handle_bus_event(
        &mut app,
        &mut remote,
        Ok(crate::bus::BusEvent::DictationCompleted {
            dictation_id: "dictation_other".to_string(),
            session_id: Some("session_other".to_string()),
            text: " dictated text".to_string(),
            mode: crate::protocol::TranscriptMode::Append,
        }),
    ));

    assert!(app.dictation_in_flight);
    assert_eq!(app.dictation_request_id.as_deref(), Some("dictation_123"));
}

#[test]
fn test_handle_server_event_transcript_send_prefixes_user_message() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Transcript {
            text: "dictated hello".to_string(),
            mode: crate::protocol::TranscriptMode::Send,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(
        matches!(app.messages.last(), Some(message) if matches!(message.role, crate::message::Role::User))
    );
    assert!(matches!(
        app.messages.last().and_then(|message| message.content.last()),
        Some(crate::message::ContentBlock::Text { text, .. }) if text == "[transcription] dictated hello"
    ));
    assert!(
        app.pending_turn,
        "local transcript send should use normal submit path"
    );
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
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_same"));
    assert_eq!(app.connection_type.as_deref(), Some("websocket"));
}

#[test]
fn test_handle_server_event_history_session_change_clears_pending_interleaves() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.remote_session_id = Some("session_old".to_string());
    app.queued_messages.push("queued later".to_string());
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(12, "acked interleave".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "session_new".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert_eq!(app.remote_session_id.as_deref(), Some("session_new"));
    assert!(app.queued_messages().is_empty());
    assert!(app.interleave_message.is_none());
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
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
fn test_handle_post_connect_dispatches_hidden_reload_followup_immediately() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_hidden_reload_followup";
    crate::tool::selfdev::ReloadContext {
        task_context: Some("Investigate queued prompt delivery after reload".to_string()),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-03-26T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        app.is_processing,
        "hidden reload continuation should dispatch immediately"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    let pending = app
        .rate_limit_pending_message
        .as_ref()
        .expect("expected pending remote message for dispatched continuation");
    assert!(pending.is_system);
    assert_eq!(pending.content, "");
    let reminder = pending
        .system_reminder
        .as_ref()
        .expect("expected hidden system reminder");
    assert!(reminder.contains("Reload succeeded (old-build → new-build)"));
    assert!(reminder.contains("Continue immediately from where you left off"));

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_clears_deferred_dispatch_before_reload_followup() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_reload_deferred_dispatch";
    crate::tool::selfdev::ReloadContext {
        task_context: Some(
            "Verify deferred dispatch does not block reload continuation".to_string(),
        ),
        version_before: "old-build".to_string(),
        version_after: "new-build".to_string(),
        session_id: session_id.to_string(),
        timestamp: "2026-04-15T00:00:00Z".to_string(),
    }
    .save()
    .expect("save reload context");

    let mut app = create_test_app();
    app.pending_queued_dispatch = true;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some(session_id),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Ready));
    assert!(
        !app.pending_queued_dispatch,
        "post-connect should clear deferred dispatch before sending reload continuation"
    );
    assert!(app.hidden_queued_system_messages.is_empty());
    assert!(
        app.is_processing,
        "reload continuation should still dispatch"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    cleanup_reload_context_file(session_id);
    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_handle_post_connect_requests_client_reload_after_server_reload_even_without_newer_binary() {
    use std::time::{Duration, SystemTime};

    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let mut app = create_test_app();
    app.client_binary_mtime = Some(SystemTime::now() + Duration::from_secs(3600));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _enter = rt.enter();
    let backend = ratatui::backend::TestBackend::new(80, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();
    app.remote_session_id = Some("session_reload_after_reconnect".to_string());

    let mut state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        server_reload_in_progress: true,
        ..Default::default()
    };

    let outcome = rt
        .block_on(super::remote::handle_post_connect(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            Some("session_reload_after_reconnect"),
        ))
        .expect("post connect should succeed");

    assert!(matches!(outcome, super::remote::PostConnectOutcome::Quit));
    assert_eq!(
        app.reload_requested.as_deref(),
        Some("session_reload_after_reconnect")
    );
    assert!(app.should_quit);

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

    app.streaming_tps_collect_output = true;

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
fn test_handle_server_event_tool_start_pauses_tps_and_excludes_hidden_output_tokens() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.streaming_tps_collect_output = true;
    app.streaming_tps_start = Some(Instant::now());

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool-1".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    assert!(!app.streaming_tps_collect_output);
    assert!(app.streaming_tps_start.is_none());

    app.handle_server_event(
        crate::protocol::ServerEvent::TokenUsage {
            input: 100,
            output: 25,
            cache_read_input: None,
            cache_creation_input: None,
        },
        &mut remote,
    );

    assert_eq!(app.streaming_total_output_tokens, 0);

    app.handle_server_event(
        crate::protocol::ServerEvent::TextDelta {
            text: "hello".to_string(),
        },
        &mut remote,
    );

    assert!(app.streaming_tps_collect_output);
    assert!(app.streaming_tps_start.is_some());
}

#[test]
fn test_handle_server_event_message_end_marks_stream_as_finalizing_without_stall_mode() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.streaming_tps_collect_output = true;

    let needs_redraw =
        app.handle_server_event(crate::protocol::ServerEvent::MessageEnd, &mut remote);

    assert!(needs_redraw);
    assert!(app.stream_message_ended);
    assert!(matches!(app.status, ProcessingStatus::Streaming));
    assert!(app.streaming_tps_collect_output);
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
    app.pending_soft_interrupt_requests
        .push((77, "pending soft interrupt".to_string()));

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
    assert_eq!(app.queued_messages(), &["queued interrupt"]);
    assert_eq!(app.pending_soft_interrupts, vec!["pending soft interrupt"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(77, "pending soft interrupt".to_string())]
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing interrupted message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Interrupted");
}

#[test]
fn test_remote_interrupted_defers_queued_followup_dispatch_by_one_cycle() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert_eq!(app.queued_messages(), &["queued later"]);
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());
}

#[test]
fn test_remote_interrupted_recovers_pending_interleaves_in_order() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Interrupted, &mut remote);

    assert!(app.pending_queued_dispatch);
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));
    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert_eq!(
        app.queued_messages(),
        &["acked interleave", "unsent interleave", "queued later"]
    );
    assert!(!app.is_processing);

    app.pending_queued_dispatch = false;
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(
        user_messages,
        vec!["acked interleave", "unsent interleave", "queued later"]
    );
}

#[test]
fn test_remote_done_recovers_stranded_soft_interrupt_as_queued_followup() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(42);
    app.pending_soft_interrupts = vec!["late interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(55, "late interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

    assert!(!app.is_processing);
    assert_eq!(app.pending_soft_interrupts, vec!["late interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(55, "late interleave".to_string())]
    );
    assert_eq!(app.queued_messages(), &["queued later"]);

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_some());

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(user_messages, vec!["late interleave", "queued later"]);
}

#[test]
fn test_remote_done_auto_pokes_again_when_todos_remain() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        crate::todo::save_todos(
            &app.session.id,
            &[crate::todo::TodoItem {
                id: "todo-1".to_string(),
                content: "Continue working".to_string(),
                status: "pending".to_string(),
                priority: "high".to_string(),
                blocked_by: Vec::new(),
                assigned_to: None,
            }],
        )
        .expect("save todos");

        app.is_remote = true;
        app.auto_poke_incomplete_todos = true;
        app.is_processing = true;
        app.status = ProcessingStatus::Streaming;
        app.current_message_id = Some(42);

        let needs_redraw =
            app.handle_server_event(crate::protocol::ServerEvent::Done { id: 42 }, &mut remote);

        assert!(needs_redraw);
        assert!(app.pending_queued_dispatch);
        assert_eq!(app.queued_messages().len(), 1);
        assert!(app.queued_messages()[0].contains("Please continue your work"));
    });
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
fn test_handle_server_event_remote_observe_tracks_tool_exec_and_done() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.input = "/observe on".to_string();
    app.submit_input();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("observe"));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolStart {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolInput {
            delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
        },
        &mut remote,
    );
    app.handle_server_event(
        crate::protocol::ServerEvent::ToolExec {
            id: "tool_read".to_string(),
            name: "read".to_string(),
        },
        &mut remote,
    );

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(
        page.content
            .contains("Latest tool call emitted by the model")
    );
    assert!(page.content.contains("`read`"));
    assert!(page.content.contains("src/main.rs"));

    app.handle_server_event(
        crate::protocol::ServerEvent::ToolDone {
            id: "tool_read".to_string(),
            name: "read".to_string(),
            output: "1 fn main() {}".to_string(),
            error: None,
        },
        &mut remote,
    );

    let page = app.side_panel.focused_page().expect("missing observe page");
    let token_label =
        crate::util::format_approx_token_count(crate::util::estimate_tokens("1 fn main() {}"));
    assert!(page.content.contains("Latest tool result added to context"));
    assert!(page.content.contains("Status: completed"));
    assert!(page.content.contains("Returned to context"));
    assert!(page.content.contains(&token_label));
    assert!(page.content.contains("1 fn main() {}"));
}

#[test]
fn test_handle_remote_event_redraws_observe_tool_exec_immediately() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut state = super::remote::RemoteRunState::default();

    app.input = "/observe on".to_string();
    app.submit_input();

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolStart {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool start should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolInput {
                delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
            }),
        ))
        .expect("tool input should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolExec {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool exec should succeed");
    assert!(needs_redraw, "observe tool exec should request redraw");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let text = buffer_to_text(&terminal);
    assert!(
        text.contains("Latest tool call emitted by the"),
        "observe tool exec should redraw immediately:\n{text}"
    );
    assert!(text.contains("Tool input"));
    assert!(text.contains("src/main.rs"));
}

#[test]
fn test_handle_remote_event_redraws_observe_tool_done_immediately() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    let mut state = super::remote::RemoteRunState::default();

    app.input = "/observe on".to_string();
    app.submit_input();

    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolStart {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool start should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolInput {
                delta: r#"{"file_path":"src/main.rs","start_line":1,"end_line":10}"#.to_string(),
            }),
        ))
        .expect("tool input should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    let (_, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolExec {
                id: "tool_read".to_string(),
                name: "read".to_string(),
            }),
        ))
        .expect("tool exec should succeed");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }

    let (outcome, needs_redraw) = rt
        .block_on(super::remote::handle_remote_event(
            &mut app,
            &mut terminal,
            &mut remote,
            &mut state,
            crate::tui::backend::RemoteRead::Event(crate::protocol::ServerEvent::ToolDone {
                id: "tool_read".to_string(),
                name: "read".to_string(),
                output: "1 fn main() {}".to_string(),
                error: None,
            }),
        ))
        .expect("tool done should succeed");
    assert!(needs_redraw, "observe tool done should request redraw");
    if needs_redraw {
        terminal
            .draw(|frame| crate::tui::ui::draw(frame, &app))
            .unwrap();
    }
    assert!(matches!(
        outcome,
        super::remote::RemoteEventOutcome::Continue
    ));

    let text = buffer_to_text(&terminal);
    assert!(
        text.contains("Latest tool result added to"),
        "observe tool done should redraw immediately:\n{text}"
    );
    assert!(text.contains("Status: completed"));
    assert!(text.contains("Returned to context:"));
}

#[test]
fn test_observe_marks_large_tool_results() {
    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let tool_call = crate::message::ToolCall {
        id: "tool_big".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "large.txt"}),
        intent: None,
    };
    let output = "x".repeat(48_000);
    app.observe_tool_result(&tool_call, &output, false, Some("read"));

    let page = app.side_panel.focused_page().expect("missing observe page");
    assert!(page.content.contains("12k tok"));
    assert!(page.content.contains("[very large]"));
    assert!(!page.content.contains('🔴'));
    assert!(!page.content.contains('⚠'));
}

#[test]
fn test_observe_repaint_does_not_leave_severity_badge_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.input = "/observe on".to_string();
    app.submit_input();

    let backend = ratatui::backend::TestBackend::new(90, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let tool_call = crate::message::ToolCall {
        id: "tool_big".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "large.txt"}),
        intent: None,
    };

    let large_output = "x".repeat(48_000);
    app.observe_tool_result(&tool_call, &large_output, false, Some("read"));
    let first = render_and_snap(&app, &mut terminal);
    assert!(first.contains("[very large]"));

    app.observe_tool_result(&tool_call, "ok", false, Some("read"));
    let second = render_and_snap(&app, &mut terminal);

    assert!(!second.contains("[very large]"));
    assert!(!second.contains('🔴'));
    assert!(!second.contains('⚠'));
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
fn test_handle_server_event_ack_removes_only_matching_unacked_soft_interrupt() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(11, "first".to_string()), (22, "second".to_string())];

    app.handle_server_event(crate::protocol::ServerEvent::Ack { id: 11 }, &mut remote);

    assert_eq!(app.pending_soft_interrupts, vec!["first", "second"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(22, "second".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_keeps_other_pending_previews() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "first".to_string(),
            display_role: Some("user".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["second"]);
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_duplicate_content_keeps_later_pending_copy() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["same".to_string(), "same".to_string()];
    app.pending_soft_interrupt_requests = vec![(11, "same".to_string()), (22, "same".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "same".to_string(),
            display_role: Some("user".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["same"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(22, "same".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_unrelated_content_keeps_pending_previews() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.pending_soft_interrupts = vec!["first".to_string(), "second".to_string()];
    app.pending_soft_interrupt_requests =
        vec![(11, "first".to_string()), (22, "second".to_string())];

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "background task notice".to_string(),
            display_role: Some("system".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    assert_eq!(app.pending_soft_interrupts, vec!["first", "second"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(11, "first".to_string()), (22, "second".to_string())]
    );
}

#[test]
fn test_handle_server_event_soft_interrupt_injected_background_task_renders_card_role() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::SoftInterruptInjected {
            content: "**Background task** `abc123` · `bash` · ✓ completed · 7.1s · exit 0\n\n```text\nhello\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`".to_string(),
            display_role: Some("background_task".to_string()),
            point: "D".to_string(),
            tools_skipped: None,
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing injected background task message");
    assert_eq!(last.role, "background_task");
    assert!(last.content.contains("**Background task** `abc123`"));
}

#[test]
fn test_handle_server_event_notification_background_task_scope_uses_card_rendering() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.set_centered(true);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::Notification {
            from_session: "session_background_task_123".to_string(),
            from_name: Some("background task".to_string()),
            notification_type: crate::protocol::NotificationType::Message {
                scope: Some("background_task".to_string()),
                channel: None,
            },
            message: "**Background task** `abc123` · `bash` · ✗ failed · 7.1s · exit 1\n\n```text\n[stderr] line one\n[stderr] line two\n```\n\n_Full output:_ `bg action=\"output\" task_id=\"abc123\"`".to_string(),
        },
        &mut remote,
    );

    let last = app
        .display_messages()
        .last()
        .expect("missing background task notification message");
    assert_eq!(last.role, "background_task");

    let backend = ratatui::backend::TestBackend::new(42, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    assert!(
        text.contains("╭") && text.contains("╰"),
        "expected rounded background-task card in render, got:\n{}",
        text
    );
    assert!(
        !text.contains("◦ Background task"),
        "background-task notifications should not render as generic swarm items:\n{}",
        text
    );
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
    assert_eq!(last.title.as_deref(), Some("Connection"));
    assert!(last.content.contains("⚡ Connection lost — retrying"));
    assert!(last.content.contains("connection to server dropped"));
    assert!(
        !last.content.contains('\n'),
        "reconnect status should stay on one line: {}",
        last.content
    );
}

#[test]
fn test_handle_remote_disconnect_preserves_pending_interleaves_for_reconnect() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.current_message_id = Some(7);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(44, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    let mut state = remote::RemoteRunState::default();
    remote::handle_disconnect(&mut app, &mut state, None);

    assert!(!app.is_processing);
    assert!(app.interleave_message.is_none());
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(44, "acked interleave".to_string())]
    );

    remote.mark_history_loaded();
    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let user_messages: Vec<&str> = app
        .display_messages()
        .iter()
        .filter(|msg| msg.role == "user")
        .map(|msg| msg.content.as_str())
        .collect();
    assert_eq!(
        user_messages,
        vec!["acked interleave", "unsent interleave", "queued later"]
    );
}

#[test]
fn test_replace_display_message_content_bumps_version() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::system("old reconnect status".to_string()));
    let before = app.display_messages_version;

    assert!(app.replace_display_message_content(0, "new reconnect status".to_string()));
    assert_eq!(app.display_messages[0].content, "new reconnect status");
    assert_ne!(app.display_messages_version, before);

    let after_change = app.display_messages_version;
    assert!(app.replace_display_message_content(0, "new reconnect status".to_string()));
    assert_eq!(app.display_messages_version, after_change);
}

#[test]
fn test_replace_latest_tool_display_message_updates_latest_match_and_bumps_version() {
    let mut app = create_test_app();
    let tool_call = crate::message::ToolCall {
        id: "tool-1".to_string(),
        name: "read".to_string(),
        input: serde_json::json!({"file_path": "src/main.rs"}),
        intent: None,
    };

    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: "placeholder 1".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: Some("old title".to_string()),
        tool_data: Some(tool_call.clone()),
    });
    app.push_display_message(DisplayMessage {
        role: "tool".to_string(),
        content: "placeholder 2".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: Some(tool_call),
    });
    let before = app.display_messages_version;

    assert!(app.replace_latest_tool_display_message(
        "tool-1",
        Some("new title".to_string()),
        "final output".to_string(),
    ));
    assert_eq!(app.display_messages()[0].content, "placeholder 1");
    assert_eq!(
        app.display_messages()[0].title.as_deref(),
        Some("old title")
    );
    assert_eq!(app.display_messages()[1].content, "final output");
    assert_eq!(
        app.display_messages()[1].title.as_deref(),
        Some("new title")
    );
    assert_ne!(app.display_messages_version, before);

    let after_change = app.display_messages_version;
    assert!(app.replace_latest_tool_display_message(
        "tool-1",
        Some("new title".to_string()),
        "final output".to_string(),
    ));
    assert_eq!(app.display_messages_version, after_change);
}

#[test]
fn test_push_display_message_coalesces_repeated_single_line_system_messages() {
    let mut app = create_test_app();

    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));
    let before = app.display_messages_version;
    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));
    app.push_display_message(DisplayMessage::system(
        "✓ Reconnected successfully.".to_string(),
    ));

    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(
        app.display_messages()[0].content,
        "✓ Reconnected successfully. [×3]"
    );
    assert_ne!(app.display_messages_version, before);
}

#[test]
fn test_push_display_message_does_not_coalesce_multiline_system_messages() {
    let mut app = create_test_app();
    let message = "Reload complete\ncontinuing";

    app.push_display_message(DisplayMessage::system(message.to_string()));
    app.push_display_message(DisplayMessage::system(message.to_string()));

    assert_eq!(app.display_messages().len(), 2);
    assert_eq!(app.display_messages()[0].content, message);
    assert_eq!(app.display_messages()[1].content, message);
}

#[test]
fn test_remove_display_message_bumps_version() {
    let mut app = create_test_app();
    app.push_display_message(DisplayMessage::system(
        "temporary reconnect status".to_string(),
    ));
    let before = app.display_messages_version;

    let removed = app
        .remove_display_message(0)
        .expect("message should be removed");
    assert_eq!(removed.content, "temporary reconnect status");
    assert!(app.display_messages.is_empty());
    assert_ne!(app.display_messages_version, before);
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
            post_tokens: Some(4_321),
            tokens_saved: Some(8_024),
            duration_ms: Some(1_532),
            messages_dropped: None,
            messages_compacted: Some(24),
            summary_chars: Some(987),
            active_messages: Some(10),
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
        "📦 **Context compacted** (semantic) — older messages were summarized to stay within the context window.\n\nTook 1.5s · before ~12,345 tokens · now ~4,321 tokens (2.2% of window) · saved ~8,024 tokens · summarized 24 messages · summary 987 chars · kept 10 recent messages live"
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
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
fn test_reload_persisted_background_tasks_note_mentions_running_task() {
    let session_id = crate::id::new_id("ses_bg_note");
    let manager = crate::background::global();
    let info = manager.reserve_task_info();
    let started_at = chrono::Utc::now().to_rfc3339();
    let rt = tokio::runtime::Runtime::new().unwrap();

    rt.block_on(manager.register_detached_task(
        &info,
        "bash",
        &session_id,
        std::process::id(),
        &started_at,
        true,
        false,
    ));

    let note = reload_persisted_background_tasks_note(&session_id);

    assert!(note.contains(&info.task_id));
    assert!(note.contains("Do not rerun those commands"));
    assert!(note.contains("bg action=\"status\""));

    cleanup_background_task_files(&info.task_id);
}

#[test]
fn test_finalize_reload_reconnect_mentions_persisted_background_task() {
    let _guard = crate::storage::lock_test_env();
    let mut app = create_test_app();
    let session_id = crate::id::new_id("ses_reload_bg");
    let reload_ctx = crate::tool::selfdev::ReloadContext {
        task_context: Some("Waiting for cargo build --release".to_string()),
        version_before: "v0.1.100".to_string(),
        version_after: "abc1234".to_string(),
        session_id: session_id.clone(),
        timestamp: chrono::Utc::now().to_rfc3339(),
    };
    reload_ctx.save().expect("save reload context");

    let manager = crate::background::global();
    let info = manager.reserve_task_info();
    let started_at = chrono::Utc::now().to_rfc3339();
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(manager.register_detached_task(
        &info,
        "bash",
        &session_id,
        std::process::id(),
        &started_at,
        true,
        false,
    ));

    remote::finalize_reload_reconnect(
        &mut app,
        Some(session_id.as_str()),
        remote::ReloadReconnectHints {
            has_reload_ctx_for_session: true,
            has_client_reload_marker: false,
        },
        false,
    );

    assert_eq!(app.hidden_queued_system_messages.len(), 1);
    let continuation = &app.hidden_queued_system_messages[0];
    assert!(continuation.contains("Persisted background task(s)"));
    assert!(continuation.contains(&info.task_id));
    assert!(continuation.contains("Do not rerun those commands"));
    assert!(continuation.contains("bg action=\"output\""));

    cleanup_background_task_files(&info.task_id);
    cleanup_reload_context_file(&session_id);
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
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "# Plan\n```mermaid\nflowchart LR\nA-->B\n```".to_string(),
            updated_at_ms: 1,
        }],
    };

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_side_panel_history".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
fn test_handle_server_event_history_restores_active_resume_processing_state() {
    let _guard = crate::storage::lock_test_env();
    let mut app = App::new_for_remote(Some("ses_resume_active".to_string()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: "ses_resume_active".to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("openai".to_string()),
            provider_model: Some("gpt-5.4".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: Some(crate::protocol::SessionActivitySnapshot {
                is_processing: true,
                current_tool_name: Some("batch".to_string()),
            }),
            side_panel: crate::side_panel::SidePanelSnapshot::default(),
        },
        &mut remote,
    );

    assert!(app.is_processing());
    assert!(app.processing_started.is_some());
    assert!(app.time_since_activity().is_some());
    assert!(matches!(app.status, ProcessingStatus::RunningTool(ref name) if name == "batch"));
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
            source: crate::side_panel::SidePanelPageSource::Managed,
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
                    source: crate::side_panel::SidePanelPageSource::Managed,
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
fn test_metadata_only_history_preserves_fast_restored_startup_state() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_fast_resume_meta_42";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("resume me".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-fast-resume".to_string(),
        role: crate::message::Role::Assistant,
        content: vec![crate::message::ContentBlock::Text {
            text: "restored locally before connect".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save fast resume session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard_rt = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    app.handle_server_event(
        crate::protocol::ServerEvent::History {
            id: 1,
            session_id: session_id.to_string(),
            messages: vec![],
            images: vec![],
            provider_name: Some("openai".to_string()),
            provider_model: Some("gpt-5.4".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
            available_models: vec![],
            available_model_routes: vec![],
            mcp_servers: vec![],
            skills: vec![],
            total_tokens: None,
            all_sessions: vec![session_id.to_string()],
            client_count: Some(1),
            is_canary: Some(false),
            server_version: None,
            server_name: None,
            server_icon: None,
            server_has_update: None,
            was_interrupted: None,
            connection_type: Some("https".to_string()),
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
    assert_eq!(
        assistant_messages[0].content,
        "restored locally before connect"
    );
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id));
    assert_eq!(app.connection_type.as_deref(), Some("https"));

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
            images: vec![],
            provider_name: Some("claude".to_string()),
            provider_model: Some("claude-sonnet-4-20250514".to_string()),
            subagent_model: None,
            autoreview_enabled: None,
            autojudge_enabled: None,
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
            status_detail: None,
            upstream_provider: None,
            reasoning_effort: None,
            service_tier: None,
            compaction_mode: crate::config::CompactionMode::Reactive,
            activity: None,
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
fn test_remote_error_without_retry_recovers_pending_followups() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();
    remote.mark_history_loaded();

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
    app.current_message_id = Some(10);
    app.interleave_message = Some("unsent interleave".to_string());
    app.pending_soft_interrupts = vec!["acked interleave".to_string()];
    app.pending_soft_interrupt_requests = vec![(88, "acked interleave".to_string())];
    app.queued_messages.push("queued later".to_string());

    app.handle_server_event(
        crate::protocol::ServerEvent::Error {
            id: 10,
            message: "provider failed hard".to_string(),
            retry_after_secs: None,
        },
        &mut remote,
    );

    assert!(app.rate_limit_pending_message.is_none());
    assert!(app.interleave_message.is_none());
    assert_eq!(
        app.queued_messages(),
        &["unsent interleave", "queued later"]
    );
    assert_eq!(app.pending_soft_interrupts, vec!["acked interleave"]);
    assert_eq!(
        app.pending_soft_interrupt_requests,
        vec![(88, "acked interleave".to_string())]
    );

    rt.block_on(remote::process_remote_followups(&mut app, &mut remote));

    assert!(app.pending_soft_interrupts.is_empty());
    assert!(app.pending_soft_interrupt_requests.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.is_processing);
    assert!(matches!(app.status, ProcessingStatus::Sending));

    let last = app
        .display_messages()
        .last()
        .expect("missing error message");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "queued later");
    assert!(
        app.display_messages()
            .iter()
            .any(|m| m.role == "error" && m.content == "provider failed hard")
    );
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
fn test_remote_tui_state_prefers_cached_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_123";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");
    assert_eq!(
        crate::tui::TuiState::session_display_name(&app).as_deref(),
        Some("otter")
    );

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_falls_back_to_cached_model_after_startup_phase_clears() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_124";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.save().expect("save remote session");

    let mut app = App::new_for_remote(Some(session_id.to_string()));
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_new_for_remote_uses_startup_stub_without_loading_full_transcript() {
    let _guard = crate::storage::lock_test_env();
    let temp_home = tempfile::TempDir::new().expect("create temp home");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp_home.path());

    let session_id = "session_otter_stub_125";
    let mut session = crate::session::Session::create_with_id(
        session_id.to_string(),
        None,
        Some("remote cached model".to_string()),
    );
    session.model = Some("gpt-5.4".to_string());
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg-startup-stub".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "hello from persisted history".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save remote session");

    let app = App::new_for_remote(Some(session_id.to_string()));
    assert_eq!(app.session_id(), session_id);
    assert_eq!(app.display_messages().len(), 1);
    assert_eq!(
        app.display_messages()[0].content,
        "hello from persisted history"
    );
    assert_eq!(app.session.messages.len(), 1);
    assert_eq!(app.remote_session_id.as_deref(), Some(session_id));
    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn test_remote_tui_state_shows_connected_after_startup_phase_clears_without_model() {
    let mut app = App::new_for_remote(None);
    app.remote_session_id = Some("session_connected_123".to_string());
    app.clear_remote_startup_phase();

    assert_eq!(crate::tui::TuiState::provider_model(&app), "connected");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");
}

#[test]
fn test_remote_tui_state_hides_brief_connecting_phase_without_cached_model() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "unknown");
    crate::env::remove_var("JCODE_PROVIDER");

    let app = App::new_for_remote(None);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(crate::tui::TuiState::provider_name(&app), "");

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
fn test_remote_tui_state_prefers_configured_model_during_brief_connecting_phase() {
    let _guard = crate::storage::lock_test_env();
    let prev_model = std::env::var_os("JCODE_MODEL");
    let prev_provider = std::env::var_os("JCODE_PROVIDER");
    crate::env::set_var("JCODE_MODEL", "gpt-5.4");
    crate::env::set_var("JCODE_PROVIDER", "openai");

    let app = App::new_for_remote(None);

    assert_eq!(crate::tui::TuiState::provider_model(&app), "gpt-5.4");
    assert_eq!(crate::tui::TuiState::provider_name(&app), "openai");

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
fn test_remote_tui_state_shows_starting_server_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_server_spawning();

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "starting server…"
    );
}

#[test]
fn test_remote_tui_state_shows_loading_session_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
}

#[test]
fn test_remote_tui_state_shows_startup_elapsed_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);
    app.remote_startup_phase_started =
        Some(std::time::Instant::now() - std::time::Duration::from_secs(5));

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server… 5s"
    );
}

#[test]
fn test_remote_startup_phase_does_not_require_duplicate_status_notice() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Connecting);

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "connecting to server…"
    );
    assert_eq!(app.status_notice(), None);

    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::LoadingSession);
    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "loading session…"
    );
    assert_eq!(app.status_notice(), None);
}

#[test]
fn test_remote_tui_state_shows_reconnecting_phase_in_header() {
    let mut app = App::new_for_remote(None);
    app.set_remote_startup_phase(crate::tui::app::RemoteStartupPhase::Reconnecting { attempt: 3 });

    assert_eq!(
        crate::tui::TuiState::provider_model(&app),
        "reconnecting (3)…"
    );
}

#[test]
fn test_openai_compatible_login_preserves_profile_for_runtime_activation() {
    let mut app = create_test_app();

    app.start_login_provider(crate::provider_catalog::ZAI_LOGIN_PROVIDER);

    match app.pending_login {
        Some(crate::tui::app::PendingLogin::ApiKeyProfile {
            provider,
            openai_compatible_profile: Some(profile),
            ..
        }) => {
            assert_eq!(provider, "Z.AI");
            assert_eq!(profile.id, crate::provider_catalog::ZAI_PROFILE.id);
        }
        ref other => panic!("unexpected pending login state: {other:?}"),
    }
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

#[test]
fn test_debug_command_side_panel_latency_bench_reports_immediate_redraw() {
    let mut app = create_test_app();
    let result = app.handle_debug_command(
        r#"side-panel-latency:{"iterations":8,"warmup_iterations":2,"include_samples":false}"#,
    );
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("side-panel latency bench should return JSON");

    assert_eq!(value.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        value["summary"]["scroll_only_count"].as_u64(),
        Some(0),
        "side-panel latency bench should observe immediate redraw events"
    );
    assert_eq!(
        value["summary"]["unchanged_scroll_count"].as_u64(),
        Some(0),
        "each injected event should change effective side-pane scroll"
    );
    assert!(
        value["summary"]["latency_ms"]["p95"]
            .as_f64()
            .unwrap_or_default()
            < 16.0,
        "headless side-panel p95 latency should stay within a 60fps frame budget: {}",
        result
    );
}

#[test]
fn test_debug_command_mermaid_flicker_bench_returns_json() {
    let mut app = create_test_app();
    let result = app.handle_debug_command("mermaid:flicker-bench 8");
    let value: serde_json::Value =
        serde_json::from_str(&result).expect("flicker bench should return JSON");

    assert_eq!(value["steps"].as_u64(), Some(8));
    assert!(
        value
            .get("protocol_supported")
            .and_then(|v| v.as_bool())
            .is_some(),
        "expected protocol_supported bool in result: {}",
        result
    );
    assert!(
        value.get("deltas").is_some(),
        "expected delta counters: {}",
        result
    );
}

#[test]
fn test_remote_transcript_send_uses_remote_submission_path() {
    let mut app = create_test_app();
    app.is_remote = true;
    let rt = tokio::runtime::Runtime::new().expect("runtime");

    rt.block_on(async {
        let mut remote = crate::tui::backend::RemoteConnection::dummy();
        super::remote::apply_remote_transcript_event(
            &mut app,
            &mut remote,
            "dictated hello".to_string(),
            crate::protocol::TranscriptMode::Send,
        )
        .await
    })
    .expect("remote transcript send should succeed");

    let last = app
        .display_messages()
        .last()
        .expect("user message displayed");
    assert_eq!(last.role, "user");
    assert_eq!(last.content, "[transcription] dictated hello");
    assert!(
        app.is_processing,
        "remote send should enter processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(
        app.current_message_id.is_some(),
        "remote request id should be assigned"
    );
    assert!(
        app.last_stream_activity.is_some(),
        "remote send should start stall timer from a real send"
    );
    assert!(
        !app.pending_turn,
        "remote transcript send must not use local pending_turn path"
    );
    assert!(
        app.input.is_empty(),
        "submitted transcript should clear input"
    );
    assert!(
        app.rate_limit_pending_message.is_some(),
        "remote send should populate retry state for the in-flight request"
    );
}

#[test]
fn test_remote_review_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/review".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/review should launch split request");

    assert!(
        app.is_processing,
        "review launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Review launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Review"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_review_child".to_string(),
            new_session_name: "review_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
}

#[test]
fn test_remote_super_space_routes_next_prompt_to_new_session() {
    with_temp_jcode_home(|| {
        let mut app = create_test_app();
        app.is_remote = true;
        app.input = "hello from split".to_string();
        app.cursor_pos = app.input.len();

        let rt = tokio::runtime::Runtime::new().expect("runtime");
        let _guard = rt.enter();
        let mut remote = crate::tui::backend::RemoteConnection::dummy();

        rt.block_on(app.handle_remote_key(KeyCode::Char(' '), KeyModifiers::SUPER, &mut remote))
            .expect("Super+Space should arm routing");
        assert!(app.route_next_prompt_to_new_session);

        rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
            .expect("armed prompt should launch split request");

        assert!(!app.route_next_prompt_to_new_session);
        assert!(app.pending_split_prompt.is_some());
        assert_eq!(app.pending_split_label.as_deref(), Some("Prompt"));
        assert!(app.is_processing);
        assert!(matches!(app.status, ProcessingStatus::Sending));
        assert!(app.current_message_id.is_none());

        app.handle_server_event(
            crate::protocol::ServerEvent::SplitResponse {
                id: 1,
                new_session_id: "session_prompt_child".to_string(),
                new_session_name: "prompt_child".to_string(),
            },
            &mut remote,
        );

        let restored = App::restore_input_for_reload("session_prompt_child")
            .expect("new prompt session should have startup submission saved");
        assert_eq!(restored.input, "hello from split");
        assert!(restored.submit_on_restore);
        assert!(restored.pending_images.is_empty());
        assert!(app.pending_split_prompt.is_none());
        assert!(app.pending_split_label.is_none());
    });
}

#[test]
fn test_remote_judge_shows_processing_until_split_response() {
    let mut app = create_test_app();
    app.is_remote = true;
    app.input = "/judge".to_string();
    app.cursor_pos = app.input.len();

    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Enter, KeyModifiers::empty(), &mut remote))
        .expect("/judge should launch split request");

    assert!(
        app.is_processing,
        "judge launch should show client processing state"
    );
    assert!(matches!(app.status, ProcessingStatus::Sending));
    assert!(app.current_message_id.is_none());
    assert_eq!(app.status_notice(), Some("Judge launching".to_string()));
    assert!(app.pending_split_startup_message.is_some());
    assert_eq!(app.pending_split_label.as_deref(), Some("Judge"));
    assert!(!app.pending_split_request);

    app.handle_server_event(
        crate::protocol::ServerEvent::SplitResponse {
            id: 1,
            new_session_id: "session_judge_child".to_string(),
            new_session_name: "judge_child".to_string(),
        },
        &mut remote,
    );

    assert!(
        !app.is_processing,
        "split response should clear transient launch state"
    );
    assert!(matches!(app.status, ProcessingStatus::Idle));
    assert!(app.processing_started.is_none());
    assert!(app.pending_split_startup_message.is_none());
    assert!(app.pending_split_label.is_none());
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
    while lines.last().is_some_and(|l| l.is_empty()) {
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

fn create_error_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("Show me the last error"),
        DisplayMessage::error("permission denied while opening ~/.jcode/config.toml"),
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

fn create_tool_error_copy_test_app() -> (App, ratatui::Terminal<ratatui::backend::TestBackend>) {
    let mut app = create_test_app();
    app.display_messages = vec![
        DisplayMessage::user("Run the command"),
        DisplayMessage::tool(
            "Error: permission denied",
            crate::message::ToolCall {
                id: "tool_1".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "cat /root/secret"}),
                intent: None,
            },
        ),
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
fn test_armed_new_session_mode_shows_input_hint_and_indicator() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.input = "draft prompt".to_string();
    app.cursor_pos = app.input.len();
    app.handle_key(KeyCode::Char(' '), KeyModifiers::SUPER)
        .expect("Super+Space should arm new-session mode");

    let backend = ratatui::backend::TestBackend::new(60, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let rendered = render_and_snap(&app, &mut terminal);

    assert!(
        rendered.contains("↗ Next prompt opens a new session"),
        "rendered UI should show armed-mode hint, got:\n{}",
        rendered
    );
    assert!(
        rendered.contains("↗"),
        "rendered UI should show armed-mode indicator icon, got:\n{}",
        rendered
    );
}

#[test]
fn test_chat_native_scrollbar_hidden_when_content_fits() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    app.chat_native_scrollbar = true;
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: "short response".to_string(),
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.session.short_name = Some("test".to_string());
    app.is_processing = false;
    app.status = ProcessingStatus::Idle;

    let backend = ratatui::backend::TestBackend::new(60, 24);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    let text = render_and_snap(&app, &mut terminal);

    assert_eq!(crate::tui::ui::last_max_scroll(), 0);
    for glyph in ["╷", "╵", "╎"] {
        assert!(
            !text.contains(glyph),
            "did not expect scrollbar glyph {glyph:?} when content fits:\n{text}"
        );
    }
}

#[test]
fn test_chat_native_scrollbar_hides_scroll_counters() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(50, 12, 0, 24);
    app.chat_native_scrollbar = true;
    app.auto_scroll_paused = true;

    let _ = render_and_snap(&app, &mut terminal);
    let max_scroll = crate::tui::ui::last_max_scroll();
    assert!(
        max_scroll > 2,
        "expected scrollable content, got max_scroll={max_scroll}"
    );

    app.scroll_offset = max_scroll / 2;
    let text = render_and_snap(&app, &mut terminal);
    let scroll = app.scroll_offset.min(crate::tui::ui::last_max_scroll());
    let remaining = crate::tui::ui::last_max_scroll().saturating_sub(scroll);

    assert!(
        text.contains('╷') || text.contains('•'),
        "expected native scrollbar thumb to render:\n{text}"
    );
    assert!(
        !text.contains('╎'),
        "did not expect dotted scrollbar track to render:\n{text}"
    );
    assert!(
        !text.contains(&format!("↑{scroll}")),
        "top scroll counter should be hidden when native scrollbar is visible:\n{text}"
    );
    assert!(
        !text.contains(&format!("↓{remaining}")),
        "bottom scroll counter should be hidden when native scrollbar is visible:\n{text}"
    );
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
fn test_queued_file_activity_repaint_does_not_leave_trailing_digit_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(140, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.is_processing = true;
    app.status = ProcessingStatus::Streaming;
    app.pending_soft_interrupts = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs — amber previously read this file: read lines 1-9999"
            .to_string(),
    ];
    let first = render_and_snap(&app, &mut terminal);
    assert!(
        first.contains("1-9999"),
        "expected initial queued alert to render fully"
    );

    app.pending_soft_interrupts = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs — amber previously read this file: read lines 1-9"
            .to_string(),
    ];
    let second = render_and_snap(&app, &mut terminal);

    assert!(
        second.contains("⚠ File activity:"),
        "expected queued alert to use width-stable warning glyph, got:\n{second}"
    );
    assert!(
        !second.contains("⚠️ File activity:"),
        "queued alert should not use emoji warning presentation in repaint-sensitive UI:\n{second}"
    );
    assert!(
        second.contains("read lines 1-9"),
        "expected updated queued alert to render, got:\n{second}"
    );
    assert!(
        !second.contains("1-9999"),
        "stale trailing digits from the previous queued alert should not persist after repaint:\n{second}"
    );
}

#[test]
fn test_notification_file_activity_repaint_does_not_leave_trailing_digit_artifact() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(140, 20);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    app.status_notice = Some((
        "File activity · /home/jeremy/jcode/src/lib.rs · read lines 1-9999".to_string(),
        std::time::Instant::now(),
    ));
    let first = render_and_snap(&app, &mut terminal);
    assert!(
        first.contains("1-9999"),
        "expected initial notification to render fully"
    );

    app.status_notice = Some((
        "File activity · /home/jeremy/jcode/src/lib.rs · read lines 1-9".to_string(),
        std::time::Instant::now(),
    ));
    let second = render_and_snap(&app, &mut terminal);

    assert!(
        second.contains("read lines 1-9"),
        "expected updated notification to render, got:\n{second}"
    );
    assert!(
        !second.contains("1-9999"),
        "stale trailing digits from the previous notification should not persist after repaint:\n{second}"
    );
}

#[test]
fn test_file_activity_scroll_reproduces_trailing_nines_after_native_scroll_like_mutation() {
    let _lock = scroll_render_test_lock();

    let mut app = create_test_app();
    let backend = ratatui::backend::TestBackend::new(120, 12);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");

    let mut lines = vec![
        "⚠️ File activity: /home/jeremy/jcode/src/lib.rs — amber previously read this file: read lines 1-9"
            .to_string(),
    ];
    for idx in 1..=40 {
        lines.push(format!("filler line {idx:02}"));
    }

    app.display_messages = vec![DisplayMessage::assistant(lines.join("\n"))];
    app.bump_display_messages_version();
    app.auto_scroll_paused = true;
    app.scroll_offset = 0;

    let clean = render_and_snap(&app, &mut terminal);
    let target_row = clean
        .lines()
        .position(|line| line.contains("read lines"))
        .unwrap_or_else(|| panic!("expected file activity line to be visible, got:\n{clean}"));
    let target_line = clean.lines().nth(target_row).expect("target line text");
    let trail_start = target_line
        .find("read lines 1-9")
        .expect("expected file activity suffix")
        + "read lines 1-9".len();

    let ghost = ratatui::buffer::Buffer::with_lines(["9999"]);
    let updates = ghost
        .content()
        .iter()
        .enumerate()
        .map(|(idx, cell)| (trail_start as u16 + idx as u16, target_row as u16, cell));
    terminal
        .backend_mut()
        .draw(updates)
        .expect("inject trailing nines after file activity line");

    app.scroll_offset = 1;
    let scrolled = render_and_snap(&app, &mut terminal);

    assert!(
        scrolled.contains("9999"),
        "expected stale trailing nines to remain after scroll-like repaint:\n{scrolled}"
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
fn test_remote_shift_slash_inserts_question_mark() {
    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('/'), KeyModifiers::SHIFT, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "?");
    assert_eq!(app.cursor_pos(), 1);
}

#[test]
fn test_remote_key_event_shift_slash_inserts_question_mark() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(remote::handle_remote_key_event(
        &mut app,
        KeyEvent::new_with_kind(KeyCode::Char('/'), KeyModifiers::SHIFT, KeyEventKind::Press),
        &mut remote,
    ))
    .unwrap();

    assert_eq!(app.input(), "?");
    assert_eq!(app.cursor_pos(), 1);
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
fn test_local_alt_m_toggles_side_panel_visibility() {
    let mut app = create_test_app();
    app.side_panel = test_side_panel_snapshot("plan", "Plan");
    app.last_side_panel_focus_id = Some("plan".to_string());

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id, None);
    assert_eq!(app.status_notice(), Some("Side panel: OFF".to_string()));

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.status_notice(), Some("Side panel: Plan".to_string()));
}

#[test]
fn test_local_alt_m_falls_back_to_diagram_pane_when_side_panel_is_empty() {
    let mut app = create_test_app();
    app.side_panel = crate::side_panel::SidePanelSnapshot::default();
    app.diagram_pane_enabled = true;

    app.handle_key(KeyCode::Char('m'), KeyModifiers::ALT)
        .unwrap();

    assert!(!app.diagram_pane_enabled);
    assert_eq!(app.status_notice(), Some("Diagram pane: OFF".to_string()));
}

#[test]
fn test_remote_alt_m_toggles_side_panel_visibility() {
    let mut app = create_test_app();
    app.side_panel = test_side_panel_snapshot("plan", "Plan");
    app.last_side_panel_focus_id = Some("plan".to_string());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('m'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id, None);
    assert_eq!(app.status_notice(), Some("Side panel: OFF".to_string()));

    rt.block_on(app.handle_remote_key(KeyCode::Char('m'), KeyModifiers::ALT, &mut remote))
        .unwrap();
    assert_eq!(app.side_panel.focused_page_id.as_deref(), Some("plan"));
    assert_eq!(app.status_notice(), Some("Side panel: Plan".to_string()));
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
fn test_should_allow_reconnect_takeover_only_after_successful_attach() {
    let mut app = create_test_app();
    let state = super::remote::RemoteRunState {
        reconnect_attempts: 1,
        ..Default::default()
    };

    app.resume_session_id = Some("ses_resume_only".to_string());
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));

    app.remote_session_id = Some("ses_other".to_string());
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));

    app.remote_session_id = Some("ses_resume_only".to_string());
    assert!(super::remote::should_allow_reconnect_takeover(
        &app,
        &state,
        app.resume_session_id.as_deref(),
    ));
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app,
        &super::remote::RemoteRunState::default(),
        app.resume_session_id.as_deref(),
    ));
    assert!(!super::remote::should_allow_reconnect_takeover(
        &app, &state, None,
    ));
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
fn test_local_error_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_error_copy_test_app();

    let initial = render_and_snap(&app, &mut terminal);
    assert!(
        initial.contains("[S]"),
        "expected visible error copy badge: {}",
        initial
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.status_notice(), Some("Copied error".to_string()));

    let text = render_and_snap(&app, &mut terminal);
    assert!(
        text.contains("Copied!"),
        "expected inline copied feedback: {}",
        text
    );
}

#[test]
fn test_local_tool_error_copy_badge_shortcut_supported() {
    let _render_lock = scroll_render_test_lock();
    let (mut app, mut terminal) = create_tool_error_copy_test_app();

    let initial = render_and_snap(&app, &mut terminal);
    assert!(
        initial.contains("[S]"),
        "expected visible tool error copy badge: {}",
        initial
    );

    app.handle_key(KeyCode::Char('S'), KeyModifiers::ALT)
        .unwrap();

    assert_eq!(app.status_notice(), Some("Copied error".to_string()));

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
            text.contains("Show me some code")
                .then_some((abs_line, text))
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
            source: crate::side_panel::SidePanelPageSource::Managed,
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
fn test_ctrl_a_copies_chat_viewport_with_context_when_input_empty() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    let copied = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let copied_for_closure = copied.clone();

    let lines = (1..=40)
        .map(|idx| format!("line {idx:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 12;
    app.auto_scroll_paused = true;

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let (visible_start, visible_end) =
        crate::tui::ui::copy_viewport_visible_range().expect("visible copy range");
    let line_count = crate::tui::ui::copy_viewport_line_count().expect("line count");
    let context = 4usize;
    let expected_start = visible_start.saturating_sub(context);
    let expected_end = visible_end
        .saturating_add(context)
        .saturating_sub(1)
        .min(line_count.saturating_sub(1));
    assert!(app.select_chat_viewport_context());
    let range = app
        .normalized_copy_selection()
        .expect("expected viewport context range");
    assert_eq!(range.start.pane, crate::tui::CopySelectionPane::Chat);
    assert_eq!(range.end.pane, crate::tui::CopySelectionPane::Chat);
    assert_eq!(range.start.abs_line, expected_start);
    assert_eq!(range.end.abs_line, expected_end);
    let preselected_text = app
        .current_copy_selection_text()
        .expect("expected viewport context text");
    assert!(
        !preselected_text.trim().is_empty(),
        "viewport context selection should not be empty"
    );

    let success = app.copy_current_selection_to_clipboard_with(|text| {
        *copied_for_closure.lock().unwrap() = text.to_string();
        true
    });

    assert!(success);
    let copied_text = copied.lock().unwrap().clone();
    assert!(
        copied_text == preselected_text,
        "copied text should match selected viewport context: {copied_text:?}"
    );
    assert_eq!(app.status_notice(), Some("Copied selection".to_string()));
    assert!(!app.copy_selection_mode);
    assert!(app.copy_selection_anchor.is_none());
    assert!(app.copy_selection_cursor.is_none());
}

#[test]
fn test_alt_a_copies_chat_viewport_with_context_when_input_empty() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();

    let lines = (1..=20)
        .map(|idx| format!("line {idx:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.display_messages = vec![DisplayMessage {
        role: "assistant".to_string(),
        content: lines,
        tool_calls: vec![],
        duration_secs: None,
        title: None,
        tool_data: None,
    }];
    app.bump_display_messages_version();
    app.scroll_offset = 4;
    app.auto_scroll_paused = true;

    let backend = ratatui::backend::TestBackend::new(40, 8);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let handled = super::input::handle_alt_key(&mut app, KeyCode::Char('a'));
    assert!(handled);
    assert!(matches!(
        app.status_notice().as_deref(),
        Some("Copied viewport context")
            | Some("Failed to copy viewport context")
            | Some("Nothing visible to copy")
    ));
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
fn test_try_open_link_at_opens_clicked_url_and_sets_notice() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    crate::tui::ui::clear_copy_viewport_snapshot();
    crate::tui::ui::record_copy_viewport_snapshot(
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![0]),
        std::sync::Arc::new(vec!["Docs: https://example.com/docs".to_string()]),
        std::sync::Arc::new(vec![crate::tui::ui::WrappedLineMap {
            raw_line: 0,
            start_col: 0,
            end_col: 30,
        }]),
        0,
        1,
        Rect::new(0, 0, 80, 5),
        &[0],
    );

    let opened = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let opened_for_closure = opened.clone();

    let handled = app.try_open_link_at_with(10, 0, |url| {
        *opened_for_closure.lock().unwrap() = Some(url.to_string());
        Ok::<(), &'static str>(())
    });

    assert!(handled);
    assert_eq!(
        *opened.lock().unwrap(),
        Some("https://example.com/docs".to_string())
    );
    assert_eq!(
        app.status_notice(),
        Some("Opened link: https://example.com/docs".to_string())
    );
}

#[test]
fn test_mouse_click_in_input_moves_cursor_to_clicked_position() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");
    let next_prompt = crate::tui::ui::input_ui::next_input_prompt_number(&app);
    let prompt_len = crate::tui::ui::input_ui::input_prompt_len(&app, next_prompt) as u16;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + prompt_len + 2,
        row: input_area.y,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert_eq!(app.cursor_pos, 2);
}

#[test]
fn test_mouse_click_in_main_chat_switches_focus_from_side_panel() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_focus = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: String::new(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };

    let backend = ratatui::backend::TestBackend::new(80, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let messages_area = layout.messages_area;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: messages_area.x + messages_area.width / 2,
        row: messages_area.y + messages_area.height / 2,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert!(
        !app.diff_pane_focus,
        "clicking chat should restore chat focus"
    );
    assert_eq!(app.status_notice(), Some("Focus: chat".to_string()));
}

#[test]
fn test_mouse_click_in_input_switches_focus_from_side_panel() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app.diff_pane_focus = true;
    app.side_panel = crate::side_panel::SidePanelSnapshot {
        focused_page_id: Some("plan".to_string()),
        pages: vec![crate::side_panel::SidePanelPage {
            id: "plan".to_string(),
            title: "Plan".to_string(),
            file_path: String::new(),
            format: crate::side_panel::SidePanelPageFormat::Markdown,
            source: crate::side_panel::SidePanelPageSource::Managed,
            content: "hello".to_string(),
            updated_at_ms: 1,
        }],
    };
    app.input = "hello world".to_string();
    app.cursor_pos = app.input.len();
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(60, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");
    let next_prompt = crate::tui::ui::input_ui::next_input_prompt_number(&app);
    let prompt_len = crate::tui::ui::input_ui::input_prompt_len(&app, next_prompt) as u16;

    let handled = app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + prompt_len + 2,
        row: input_area.y,
        modifiers: KeyModifiers::empty(),
    });

    assert!(!handled, "clicks should request an immediate redraw");
    assert_eq!(app.cursor_pos, 2);
    assert!(
        !app.diff_pane_focus,
        "clicking input should restore chat focus"
    );
    assert_eq!(app.status_notice(), Some("Focus: chat".to_string()));
}

#[test]
fn test_mouse_click_in_wrapped_input_moves_cursor_to_second_visual_line() {
    let _render_lock = scroll_render_test_lock();
    let mut app = create_test_app();
    app.input = "abcdefghij".to_string();
    app.cursor_pos = 0;
    app.set_centered(false);
    app.session.short_name = Some("test".to_string());

    let backend = ratatui::backend::TestBackend::new(11, 16);
    let mut terminal = ratatui::Terminal::new(backend).expect("failed to create test terminal");
    render_and_snap(&app, &mut terminal);

    let layout = crate::tui::ui::last_layout_snapshot().expect("layout snapshot");
    let input_area = layout.input_area.expect("input area");

    app.handle_mouse_event(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: input_area.x + 4,
        row: input_area.y + 1,
        modifiers: KeyModifiers::empty(),
    });

    assert_eq!(app.cursor_pos, 5);
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
fn test_disconnected_shift_slash_inserts_question_mark() {
    let mut app = create_test_app();

    remote::handle_disconnected_key(&mut app, KeyCode::Char('/'), KeyModifiers::SHIFT).unwrap();

    assert_eq!(app.input(), "?");
    assert!(app.queued_messages().is_empty());
}

#[test]
fn test_disconnected_key_event_shift_slash_inserts_question_mark() {
    use crossterm::event::{KeyEvent, KeyEventKind};

    let mut app = create_test_app();

    remote::handle_disconnected_key_event(
        &mut app,
        KeyEvent::new_with_kind(KeyCode::Char('/'), KeyModifiers::SHIFT, KeyEventKind::Press),
    )
    .unwrap();

    assert_eq!(app.input(), "?");
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
fn test_disconnected_key_handler_runs_effort_locally() {
    let mut app = create_test_app();
    app.input = "/effort".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    let last = app
        .display_messages()
        .last()
        .expect("missing effort message");
    assert_eq!(last.role, "system");
    assert!(last.content.contains("Reasoning effort not available"));
}

#[test]
fn test_disconnected_key_handler_runs_model_picker_locally() {
    let mut app = create_test_app();
    configure_test_remote_models(&mut app);
    app.input = "/model".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    let picker = app
        .inline_interactive_state
        .as_ref()
        .expect("model picker should open");
    assert!(!picker.entries.is_empty());
    assert_eq!(picker.entries[picker.selected].name, "gpt-5.3-codex");
}

#[test]
fn test_disconnected_key_handler_runs_reload_locally() {
    use std::time::SystemTime;

    let mut app = create_test_app();
    let exe = crate::build::launcher_binary_path().expect("launcher binary path");
    let mut created = false;
    if !exe.exists() {
        if let Some(parent) = exe.parent() {
            std::fs::create_dir_all(parent).expect("create launcher dir");
        }
        std::fs::write(&exe, "test").expect("write launcher binary fixture");
        created = true;
    }

    app.client_binary_mtime = Some(SystemTime::UNIX_EPOCH);
    app.input = "/reload".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    assert!(app.reload_requested.is_some());
    assert!(app.should_quit);

    if created {
        let _ = std::fs::remove_file(&exe);
    }
}

#[test]
fn test_disconnected_key_handler_runs_debug_command_locally() {
    crate::tui::visual_debug::disable();

    let mut app = create_test_app();
    app.input = "/debug-visual off".to_string();
    app.cursor_pos = app.input.len();

    remote::handle_disconnected_key(&mut app, KeyCode::Enter, KeyModifiers::empty()).unwrap();

    assert!(app.input.is_empty());
    assert!(app.queued_messages().is_empty());
    assert_eq!(app.status_notice(), Some("Visual debug: OFF".to_string()));
    let last = app
        .display_messages()
        .last()
        .expect("missing debug message");
    assert_eq!(last.role, "system");
    assert_eq!(last.content, "Visual debugging disabled.");
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
fn test_remote_ctrl_backspace_csi_u_char_fallback_deletes_word() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.set_input_for_test("hello world again");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('\u{8}'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello world ");
    assert_eq!(app.cursor_pos(), "hello world ".len());
}

#[test]
fn test_remote_ctrl_h_does_not_insert_text() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let _guard = rt.enter();
    let mut app = create_test_app();
    app.set_input_for_test("hello");
    let mut remote = crate::tui::backend::RemoteConnection::dummy();

    rt.block_on(app.handle_remote_key(KeyCode::Char('h'), KeyModifiers::CONTROL, &mut remote))
        .unwrap();

    assert_eq!(app.input(), "hello");
    assert_eq!(app.cursor_pos(), "hello".len());
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
fn test_full_redraw_clears_out_of_band_backend_artifacts_after_native_scroll_like_mutation() {
    let _lock = scroll_render_test_lock();

    let (mut app, mut terminal) = create_scroll_test_app(60, 12, 0, 24);
    app.auto_scroll_paused = true;
    app.scroll_offset = 6;
    let clean = render_and_snap(&app, &mut terminal);

    let width = terminal.backend().buffer().area.width;
    let ghost = ratatui::buffer::Buffer::with_lines([
        "ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ",
    ]);
    let updates = ghost
        .content()
        .iter()
        .enumerate()
        .map(|(idx, cell)| ((idx as u16) % width, (idx as u16) / width, cell));
    terminal
        .backend_mut()
        .draw(updates)
        .expect("inject backend artifact");

    let stale = buffer_to_text(&terminal);
    assert!(
        stale.contains("ZZZZ"),
        "expected injected backend artifact before redraw, got:\n{stale}"
    );

    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("normal redraw after backend mutation");
    let still_stale = buffer_to_text(&terminal);
    assert!(
        still_stale.contains("ZZZZ"),
        "without a forced full redraw, ratatui diffing should leave the injected artifact in place"
    );

    app.request_full_redraw();
    assert!(app.force_full_redraw, "full redraw flag should be armed");
    terminal.clear().expect("test backend clear should succeed");
    app.force_full_redraw = false;
    terminal
        .draw(|f| crate::tui::ui::draw(f, &app))
        .expect("forced full redraw should succeed");
    let repaired = buffer_to_text(&terminal);
    assert_eq!(
        repaired, clean,
        "forced full redraw should restore the expected frame and remove stale backend artifacts"
    );
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
