// TUI prompt rebuild must use the session working dir, not process cwd.
//
// Regression: `build_system_prompt_split` passed `None` as the working dir,
// so project AGENTS.md / PROGRESS.md loaded from "." (the client process
// cwd) and the context report showed zeros for every session whose project
// differs from wherever the user launched the client.

struct SlotProvider;

#[async_trait::async_trait]
impl Provider for SlotProvider {
    async fn complete(
        &self,
        _messages: &[Message],
        _tools: &[crate::message::ToolDefinition],
        _system: &str,
        _resume_session_id: Option<&str>,
    ) -> Result<crate::provider::EventStream> {
        unimplemented!("this test never completes a call")
    }

    fn name(&self) -> &str {
        "test"
    }

    fn fork(&self) -> Arc<dyn Provider> {
        Arc::new(SlotProvider)
    }
}

#[test]
fn tui_prompt_rebuild_uses_session_working_dir() {
    ensure_test_jcode_home_if_unset();
    let project = tempfile::TempDir::new().expect("temp project dir");
    std::fs::write(project.path().join("AGENTS.md"), "project instructions marker")
        .expect("write AGENTS.md");
    std::fs::write(project.path().join("PROGRESS.md"), "progress marker")
        .expect("write PROGRESS.md");

    let provider: Arc<dyn Provider> = Arc::new(SlotProvider);
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let registry = rt.block_on(Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    app.session.working_dir = Some(project.path().to_string_lossy().into_owned());

    let _split = app.build_system_prompt_split(None);
    assert!(
        app.context_info.has_project_agents_md,
        "session project AGENTS.md must load from the session working dir, not process cwd"
    );
    assert!(
        app.context_info.has_project_progress_md,
        "session project PROGRESS.md must load from the session working dir, not process cwd"
    );
    assert!(
        app.context_info.project_agents_md_chars > 0,
        "loaded AGENTS.md must report nonzero chars"
    );
}

#[test]
fn tui_history_sync_refreshes_agents_report_flags() {
    // Remote clients never run the per-turn prompt rebuild (turns execute on
    // the server), so the `/context` report would stay at the constructor
    // default and show zeros. The History handler mirrors the presence flags
    // from the session working dir so the report reflects the project files.
    ensure_test_jcode_home_if_unset();
    let project = tempfile::TempDir::new().expect("temp project dir");
    std::fs::write(project.path().join("AGENTS.md"), "project instructions marker")
        .expect("write AGENTS.md");
    std::fs::write(project.path().join("PROGRESS.md"), "progress marker")
        .expect("write PROGRESS.md");

    let provider: Arc<dyn Provider> = Arc::new(SlotProvider);
    let rt = tokio::runtime::Runtime::new().expect("test runtime");
    let registry = rt.block_on(Registry::new(provider.clone()));
    let mut app = App::new_for_test_harness(provider, registry);
    assert!(
        !app.context_info.has_project_agents_md,
        "precondition: fresh app reports no project AGENTS.md"
    );
    app.session.working_dir = Some(project.path().to_string_lossy().into_owned());

    app.refresh_agents_context_info();
    assert!(
        app.context_info.has_project_agents_md,
        "History sync must set the project AGENTS.md flag from the session working dir"
    );
    assert!(
        app.context_info.has_project_progress_md,
        "History sync must set the project PROGRESS.md flag from the session working dir"
    );
}
