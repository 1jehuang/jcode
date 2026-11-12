use super::*;
use crate::tool::{ToolContext, ToolExecutionMode};

struct TestEnvironment(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl TestEnvironment {
    fn new(home: &std::path::Path) -> Self {
        let keys = [
            "JCODE_HOME",
            "JCODE_TOOLS",
            "JCODE_DISABLED_TOOLS",
            "JCODE_TOOL_PROFILE",
            "JCODE_DISABLE_BASE_TOOLS",
        ];
        let saved = keys
            .iter()
            .map(|key| (*key, std::env::var_os(key)))
            .collect();
        for key in keys {
            crate::env::remove_var(key);
        }
        crate::env::set_var("JCODE_HOME", home);
        crate::config::Config::invalidate_cache();
        Self(saved)
    }
}

impl Drop for TestEnvironment {
    fn drop(&mut self) {
        for (key, value) in &self.0 {
            if let Some(value) = value {
                crate::env::set_var(key, value);
            } else {
                crate::env::remove_var(key);
            }
        }
        crate::config::Config::invalidate_cache();
    }
}

fn checkout(root: &std::path::Path) -> std::path::PathBuf {
    let repo = root.join("renamed-desktop");
    std::fs::create_dir_all(repo.join("crates/jcode-desktop-ui/src")).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"jcode-desktop\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(
        repo.join("crates/jcode-desktop-ui/Cargo.toml"),
        "[package]\nname = \"jcode-desktop-ui\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    repo
}

#[tokio::test]
async fn desktop_selfdev_is_automatic_separate_and_restored() {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir().unwrap();
    let _env = TestEnvironment::new(home.path());
    let repo = checkout(home.path());
    let cwd = repo.join("crates/jcode-desktop-ui/src");
    let provider: Arc<dyn Provider> = Arc::new(NativeAutoCompactionProvider);
    let registry = Registry::new(provider.clone()).await;
    let mut agent =
        Agent::new_with_initial_working_dir(provider.clone(), registry.clone(), cwd.to_str());
    assert!(agent.is_desktop_selfdev());
    assert!(
        !agent.is_canary(),
        "Desktop must not opt into CLI binary reloads"
    );
    let definitions = agent.tool_definitions().await;
    assert!(definitions.iter().any(|t| t.name == "desktop_selfdev"));
    for name in ["selfdev", "debug_socket", "jcode_docs"] {
        assert!(!definitions.iter().any(|t| t.name == name));
        assert!(agent.validate_tool_allowed(name).is_err());
    }
    agent.validate_tool_allowed("desktop_selfdev").unwrap();
    let prompt = agent.build_system_prompt_split(None);
    assert!(
        prompt
            .static_part
            .contains("Jcode Desktop Self-Development Mode")
    );
    assert!(!prompt.static_part.contains("selfdev build target=tui"));

    // The central dispatch guard also covers batch and direct API invocations.
    for name in ["selfdev", "debug_socket", "jcode_docs"] {
        let result = registry
            .execute(
                name,
                serde_json::json!({"action": "status"}),
                ToolContext {
                    session_id: agent.session_id().to_string(),
                    message_id: "test".into(),
                    tool_call_id: name.into(),
                    working_dir: Some(cwd.clone()),
                    stdin_request_tx: None,
                    graceful_shutdown_signal: None,
                    execution_mode: ToolExecutionMode::Direct,
                },
            )
            .await;
        assert!(result.unwrap_err().to_string().contains("Desktop"));
    }

    agent.session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text: "Persist Desktop mode regression fixture".into(),
            cache_control: None,
        }],
    );
    agent.session.save().unwrap();
    let session_id = agent.session_id().to_string();
    drop(agent);
    let mut restored = Agent::new(provider, registry);
    restored.restore_session(&session_id).unwrap();
    assert!(restored.is_desktop_selfdev());
    assert!(
        restored
            .tool_definitions()
            .await
            .iter()
            .any(|t| t.name == "desktop_selfdev")
    );
    assert!(
        restored
            .build_system_prompt_split(None)
            .static_part
            .contains("Jcode Desktop Self-Development Mode")
    );

    // A working-directory change must invalidate the previously locked surface.
    restored.set_working_dir_for_pending_context(Some(home.path().display().to_string()));
    assert!(!restored.is_desktop_selfdev());
    let ordinary = restored.tool_definitions().await;
    assert!(
        !ordinary
            .iter()
            .any(|t| t.name == "desktop_selfdev" || t.name == "selfdev")
    );
    assert!(ordinary.iter().any(|t| t.name == "jcode_docs"));
    assert!(restored.validate_tool_allowed("desktop_selfdev").is_err());
    assert!(
        !restored
            .build_system_prompt_split(None)
            .static_part
            .contains("Jcode Desktop Self-Development Mode")
    );

    restored.set_canary("cli-regression");
    let cli = restored.tool_definitions().await;
    assert!(cli.iter().any(|t| t.name == "selfdev"));
    assert!(cli.iter().any(|t| t.name == "debug_socket"));
    assert!(!cli.iter().any(|t| t.name == "desktop_selfdev"));
    assert!(
        restored
            .build_system_prompt_split(None)
            .static_part
            .contains("selfdev build target=tui")
    );
}
