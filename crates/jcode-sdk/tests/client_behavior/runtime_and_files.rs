use super::*;

#[test]
fn runtime_commands_revalidate_a_missing_hello_capability_without_retrying() {
    use jcode_harness_api::RuntimeCommand;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let client = fake_harness(move |frame, writer| {
        let ApiRequest::RuntimeCommand {
            session_id,
            command,
            args,
        } = &frame.request
        else {
            panic!("unexpected request: {:?}", frame.request);
        };
        seen.fetch_add(1, Ordering::SeqCst);
        assert_eq!(session_id, "runtime-session");
        assert_eq!(*command, RuntimeCommand::Memory);
        assert_eq!(args, &["off"]);
        reply(
            frame,
            ApiEvent::RuntimeCommandResult {
                session_id: session_id.clone(),
                command: *command,
                data: serde_json::json!({"enabled":false}),
                message: "Memory off".into(),
            },
            writer,
        );
    });
    assert!(!client.supports("runtime_commands"));
    let result = client
        .runtime_command(
            "runtime-session",
            RuntimeCommand::Memory,
            vec!["off".into()],
        )
        .unwrap();
    assert_eq!(result.data, serde_json::json!({"enabled":false}));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn runtime_commands_preserve_an_old_bridge_rejection_without_retrying() {
    use jcode_harness_api::{ErrorCode, RuntimeCommand};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let client = fake_harness(move |frame, writer| {
        assert!(matches!(frame.request, ApiRequest::RuntimeCommand { .. }));
        seen.fetch_add(1, Ordering::SeqCst);
        reply(
            frame,
            ApiEvent::Error {
                code: ErrorCode::UnknownRequest,
                message: "Upgrade the runtime host with jcode server reload".into(),
            },
            writer,
        );
    });
    let error = client
        .runtime_command("runtime-session", RuntimeCommand::Skills, vec![])
        .unwrap_err();
    assert_eq!(
        error.kind,
        jcode_sdk::ErrorKind::Harness(ErrorCode::UnknownRequest)
    );
    assert!(error.message.contains("jcode server reload"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// GA session-management, runtime, credential, and file methods must preserve
/// the stable protocol shapes while returning ergonomic SDK-owned values.
#[test]
fn ga_runtime_and_file_methods_map_requests_and_typed_replies() {
    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = std::sync::Arc::clone(&requests);
    let routes = vec![
        ModelRouteInfo {
            usage: Some(jcode_sdk::ModelUsage {
                count: 7,
                last_used_unix_secs: Some(100),
                tracking_started_unix_secs: Some(10),
                selection_count: 3,
                last_selected_unix_secs: Some(5),
            }),
            model: "claude".to_string(),
            provider: "anthropic".to_string(),
            api_method: "messages".to_string(),
            available: true,
            detail: "ready".to_string(),
        },
        ModelRouteInfo {
            usage: None,
            model: "gemini".to_string(),
            provider: "google".to_string(),
            api_method: "generate_content".to_string(),
            available: true,
            detail: "ready".to_string(),
        },
    ];
    let reply_routes = routes.clone();
    let client = fake_harness(move |frame, writer| {
        seen.lock()
            .expect("request log")
            .push(frame.request.clone());
        let event = match &frame.request {
            ApiRequest::ArchiveSession { .. }
            | ApiRequest::RestoreSession { .. }
            | ApiRequest::SetRetentionPolicy { .. }
            | ApiRequest::NotifyAuthChanged { .. }
            | ApiRequest::InvalidateUsage { .. } => ApiEvent::Ok,
            ApiRequest::Ping => ApiEvent::Pong,
            ApiRequest::GetRuntimeInfo { .. } => ApiEvent::RuntimeInfo {
                session_id: "s1".to_string(),
                provider: Some("anthropic".to_string()),
                model: Some("claude".to_string()),
                reasoning_effort: Some("high".to_string()),
                auth_method: Some("oauth".to_string()),
                routes: reply_routes.clone(),
            },
            ApiRequest::SetApiKey { provider, .. } => ApiEvent::CredentialUpdated {
                provider: provider.clone(),
                configured: true,
            },
            ApiRequest::ClearApiKey { provider } => ApiEvent::CredentialUpdated {
                provider: provider.clone(),
                configured: false,
            },
            ApiRequest::ReadFile { .. } => ApiEvent::FileContent {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                content: "hello".to_string(),
                size: 8,
                truncated: true,
            },
            ApiRequest::FindFiles { .. } => ApiEvent::Files {
                session_id: "s1".to_string(),
                paths: vec!["src/a.rs".to_string()],
            },
            ApiRequest::SearchText { .. } => ApiEvent::TextMatches {
                session_id: "s1".to_string(),
                matches: vec![TextMatch {
                    path: "src/a.rs".to_string(),
                    line: 2,
                    column: 3,
                    preview: "  hello".to_string(),
                }],
            },
            ApiRequest::FileStatus { .. } => ApiEvent::FileStatus {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                exists: true,
                kind: "file".to_string(),
                size: Some(8),
                modified_ms: Some(123),
            },
            other => panic!("unexpected request: {other:?}"),
        };
        reply(frame, event, writer);
    });

    client.archive_session("s1").expect("archive");
    client.restore_session("s1").expect("restore");
    client
        .set_retention_policy(Some(30))
        .expect("retention policy");

    let runtime = client.get_runtime_info("s1").expect("runtime info");
    assert_eq!(runtime.server, "fake-harness/1.0");
    assert_eq!(runtime.protocol_version, API_VERSION_MAJOR);
    assert_eq!(runtime.capabilities, ["sessions"]);
    assert!(runtime.healthy);
    assert_eq!(runtime.session_id, "s1");
    assert_eq!(runtime.provider.as_deref(), Some("anthropic"));
    assert_eq!(runtime.model.as_deref(), Some("claude"));
    assert_eq!(runtime.providers, ["anthropic", "google"]);
    assert_eq!(runtime.routes, routes);

    client.set_api_key("gemini-api", "secret").expect("set key");
    client.clear_api_key("jcode").expect("clear key");
    client.notify_auth_changed("openai").expect("refresh OAuth");
    client
        .invalidate_usage("claude", Some("claude-otter"))
        .expect("invalidate usage");

    let content = client
        .read_file("s1", "src/a.rs", Some(5))
        .expect("read file");
    assert_eq!(content.path, "src/a.rs");
    assert_eq!(content.content, "hello");
    assert_eq!(content.size, 8);
    assert!(content.truncated);
    assert_eq!(
        client
            .find_files("s1", "a.rs", Some(4))
            .expect("find files"),
        ["src/a.rs"]
    );
    assert_eq!(
        client
            .search_text(
                "s1",
                "hello",
                SearchTextOptions {
                    path: Some("src".to_string()),
                    limit: Some(2),
                },
            )
            .expect("search text"),
        [TextMatch {
            path: "src/a.rs".to_string(),
            line: 2,
            column: 3,
            preview: "  hello".to_string(),
        }]
    );
    let status = client.file_status("s1", "src/a.rs").expect("file status");
    assert_eq!(status.path, "src/a.rs");
    assert!(status.exists);
    assert_eq!(status.kind, "file");
    assert_eq!(status.size, Some(8));
    assert_eq!(status.modified_ms, Some(123));

    assert_eq!(
        *requests.lock().expect("request log"),
        vec![
            ApiRequest::ArchiveSession {
                session_id: "s1".to_string(),
            },
            ApiRequest::RestoreSession {
                session_id: "s1".to_string(),
            },
            ApiRequest::SetRetentionPolicy {
                archive_after_days: Some(30),
            },
            ApiRequest::Ping,
            ApiRequest::GetRuntimeInfo {
                session_id: "s1".to_string(),
            },
            ApiRequest::SetApiKey {
                provider: "gemini-api".to_string(),
                api_key: "secret".to_string(),
            },
            ApiRequest::ClearApiKey {
                provider: "jcode".to_string(),
            },
            ApiRequest::NotifyAuthChanged {
                provider: "openai".to_string(),
            },
            ApiRequest::InvalidateUsage {
                provider: "claude".to_string(),
                account_label: Some("claude-otter".to_string()),
            },
            ApiRequest::ReadFile {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
                max_bytes: Some(5),
            },
            ApiRequest::FindFiles {
                session_id: "s1".to_string(),
                query: "a.rs".to_string(),
                limit: Some(4),
            },
            ApiRequest::SearchText {
                session_id: "s1".to_string(),
                query: "hello".to_string(),
                path: Some("src".to_string()),
                limit: Some(2),
            },
            ApiRequest::FileStatus {
                session_id: "s1".to_string(),
                path: "src/a.rs".to_string(),
            },
        ]
    );
}
