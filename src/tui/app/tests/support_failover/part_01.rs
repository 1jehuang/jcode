use super::*;
use crate::bus::{
    BackgroundTaskCompleted, BackgroundTaskProgress, BackgroundTaskProgressEvent,
    BackgroundTaskProgressKind, BackgroundTaskProgressSource, BackgroundTaskStatus, BusEvent,
    ClientMaintenanceAction, InputShellCompleted, SessionUpdateStatus,
};
use crate::tui::TuiState;
use ratatui::backend::Backend;
use ratatui::layout::Rect;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

#[derive(Clone)]
struct RefreshSummaryProvider {
    summary: crate::provider::ModelCatalogRefreshSummary,
}

#[derive(Clone)]
struct OpenRouterSpecCaptureProvider {
    set_model_calls: StdArc<StdMutex<Vec<String>>>,
}

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

#[async_trait::async_trait]
impl Provider for RefreshSummaryProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("RefreshSummaryProvider")
    }

    fn name(&self) -> &str {
        "refresh-summary"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }

    async fn refresh_model_catalog(&self) -> Result<crate::provider::ModelCatalogRefreshSummary> {
        Ok(self.summary.clone())
    }
}

#[async_trait::async_trait]
impl Provider for OpenRouterSpecCaptureProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("OpenRouterSpecCaptureProvider")
    }

    fn name(&self) -> &str {
        "openrouter-spec-capture"
    }

    fn model(&self) -> String {
        "gpt-5.4".to_string()
    }

    fn model_routes(&self) -> Vec<crate::provider::ModelRoute> {
        vec![crate::provider::ModelRoute {
            model: "gpt-5.4".to_string(),
            provider: "OpenAI".to_string(),
            api_method: "openrouter".to_string(),
            available: true,
            detail: "cached route".to_string(),
            cheapness: None,
        }]
    }

    fn available_providers_for_model(&self, model: &str) -> Vec<String> {
        if model == "gpt-5.4" || model == "openai/gpt-5.4" {
            vec!["auto".to_string(), "OpenAI".to_string()]
        } else {
            Vec::new()
        }
    }

    fn available_efforts(&self) -> Vec<&'static str> {
        vec!["high"]
    }

    fn reasoning_effort(&self) -> Option<String> {
        Some("high".to_string())
    }

    fn set_reasoning_effort(&self, _effort: &str) -> Result<()> {
        Ok(())
    }

    fn set_model(&self, model: &str) -> Result<()> {
        self.set_model_calls.lock().unwrap().push(model.to_string());
        Ok(())
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(self.clone())
    }
}

fn create_test_app() -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(MockProvider);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn wait_for_model_picker_load(app: &mut App) {
    let start = Instant::now();
    while app.pending_model_picker_load.is_some() {
        app.poll_model_picker_load();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "timed out waiting for async model picker load"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn create_refresh_summary_test_app(summary: crate::provider::ModelCatalogRefreshSummary) -> App {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let provider: Arc<dyn Provider> = Arc::new(RefreshSummaryProvider { summary });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    app
}

fn create_openrouter_spec_capture_test_app() -> (App, StdArc<StdMutex<Vec<String>>>) {
    ensure_test_jcode_home_if_unset();
    clear_persisted_test_ui_state();
    crate::tui::ui::clear_test_render_state_for_tests();

    let set_model_calls = StdArc::new(StdMutex::new(Vec::new()));
    let provider: Arc<dyn Provider> = Arc::new(OpenRouterSpecCaptureProvider {
        set_model_calls: set_model_calls.clone(),
    });
    let rt = tokio::runtime::Runtime::new().unwrap();
    let registry = rt.block_on(crate::tool::Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.queue_mode = false;
    app.diff_mode = crate::config::DiffDisplayMode::Inline;
    (app, set_model_calls)
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
    app.ensure_provider_messages_hydrated();
    let before = app.messages.len();
    app.add_provider_message(Message::user("hello"));
    assert_eq!(app.messages.len(), before + 1);
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
