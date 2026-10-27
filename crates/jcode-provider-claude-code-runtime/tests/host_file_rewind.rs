//! Actual child MCP request -> host history journal -> byte restoration.
//! Separate test executable so JCODE_HOME never races another test.
#![cfg(unix)]
use futures::StreamExt;
use jcode_message_types::{Message, StreamEvent, ToolDefinition};
use jcode_provider_claude_code_runtime::{ClaudeCodeProvider, settings::ClaudeCodeSettings};
use jcode_provider_core::{NativeToolResult, Provider};
use serde_json::json;
use std::{path::Path, time::Duration};

#[tokio::test]
async fn genuine_child_mcp_write_is_restored_from_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let old_home = std::env::var_os("JCODE_HOME");
    struct Home(Option<std::ffi::OsString>);
    impl Drop for Home {
        fn drop(&mut self) {
            match self.0.take() {
                Some(home) => jcode_base::env::set_var("JCODE_HOME", home),
                None => jcode_base::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _home = Home(old_home);
    jcode_base::env::set_var("JCODE_HOME", dir.path().join("home"));
    let wrapper = dir.path().join("claude");
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fake_claude.py");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nFAKE_CLAUDE_STATE='{}' exec python3 '{}' \"$@\"\n",
            dir.path().display(),
            script.display()
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    let provider = ClaudeCodeProvider::with_settings(ClaudeCodeSettings {
        binary: wrapper.display().to_string(),
        ..Default::default()
    });
    provider.set_host_tool_executor_available(true);
    let session = format!("mcp-rewind-{}", uuid::Uuid::new_v4());
    provider.bind_session(&session);
    let path = dir.path().join("tracked.txt");
    std::fs::write(&path, b"before MCP write").unwrap();
    let checkpoint = jcode_base::file_history::checkpoint(&session, 0, "write request").unwrap();
    let input = json!({"file_path": path, "content": "after MCP write"});
    let mut stream = provider
        .complete(
            &[Message::user(&format!("host-write {input}"))],
            &[ToolDefinition::new(
                "write",
                "Tracked write",
                json!({"type":"object"}),
            )],
            "Test tracked host editor",
            None,
        )
        .await
        .unwrap();
    let mut calls = 0;
    loop {
        let event = tokio::time::timeout(Duration::from_secs(15), stream.next())
            .await
            .expect("child timed out")
            .expect("missing MessageEnd")
            .unwrap();
        match event {
            StreamEvent::NativeToolCall {
                request_id,
                tool_name,
                input,
            } => {
                assert_eq!(tool_name, "write");
                assert_eq!(
                    std::fs::read(&path).unwrap(),
                    b"before MCP write",
                    "MCP request is a pre-write barrier"
                );
                // This exercises the same journal API as Registry's write tool,
                // not a post-hoc observation or a filesystem watcher.
                jcode_base::file_history::tracked_write(
                    &session,
                    Path::new(input["file_path"].as_str().unwrap()),
                    input["content"].as_str().unwrap().as_bytes(),
                )
                .await
                .unwrap();
                calls += 1;
                provider
                    .native_result_sender()
                    .unwrap()
                    .send(NativeToolResult::success(request_id, "written".into()))
                    .await
                    .unwrap();
            }
            StreamEvent::MessageEnd { .. } => break,
            StreamEvent::Error { message, .. } => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(calls, 1);
    assert_eq!(std::fs::read(&path).unwrap(), b"after MCP write");
    assert_eq!(
        jcode_base::file_history::restore(&session, &checkpoint.id).unwrap(),
        1
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"before MCP write");
    provider.reset_conversation_state().await.unwrap();
}
