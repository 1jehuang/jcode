use super::*;

#[test]
fn server_logging_enabled_defaults_on_and_respects_falsey_env() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("JCODE_RUNTIME_MEMORY_LOG");

    crate::env::remove_var("JCODE_RUNTIME_MEMORY_LOG");
    assert!(server_logging_enabled());

    crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "0");
    assert!(!server_logging_enabled());

    crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "false");
    assert!(!server_logging_enabled());

    crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", "1");
    assert!(server_logging_enabled());

    if let Some(prev) = prev {
        crate::env::set_var("JCODE_RUNTIME_MEMORY_LOG", prev);
    } else {
        crate::env::remove_var("JCODE_RUNTIME_MEMORY_LOG");
    }
}

#[test]
fn append_server_sample_writes_jsonl_under_memory_logs_dir() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let sample = ServerRuntimeMemorySample {
        schema_version: 2,
        kind: "process".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        timestamp_ms: Utc::now().timestamp_millis(),
        source: "test".to_string(),
        trigger: RuntimeMemoryLogTrigger {
            category: "test".to_string(),
            reason: "unit".to_string(),
            session_id: None,
            detail: None,
        },
        sampling: RuntimeMemoryLogSampling::default(),
        server: ServerRuntimeMemoryServer {
            id: "server_test".to_string(),
            name: "test".to_string(),
            icon: "🧪".to_string(),
            version: "v0".to_string(),
            git_hash: "deadbeef".to_string(),
            uptime_secs: 1,
        },
        process: crate::process_memory::ProcessMemorySnapshot::default(),
        process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
        clients: ServerRuntimeMemoryClients { connected_count: 0 },
        sessions: None,
        background: ServerRuntimeMemoryBackground { task_count: 0 },
        embeddings: ServerRuntimeMemoryEmbeddings {
            model_available: false,
            stats: crate::embedding::stats(),
        },
    };

    let path = append_server_sample(&sample).expect("append server sample");
    assert!(path.exists(), "log path should exist: {}", path.display());

    let content = std::fs::read_to_string(&path).expect("read log file");
    let line = content.lines().last().expect("jsonl line");
    let parsed: serde_json::Value = serde_json::from_str(line).expect("parse json line");
    assert_eq!(parsed["source"], "test");
    assert_eq!(parsed["server"]["id"], "server_test");
    assert_eq!(parsed["kind"], "process");

    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn append_client_sample_writes_jsonl_under_memory_logs_dir() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let sample = ClientRuntimeMemorySample {
        schema_version: 2,
        kind: "process".to_string(),
        timestamp: Utc::now().to_rfc3339(),
        timestamp_ms: Utc::now().timestamp_millis(),
        source: "test".to_string(),
        trigger: RuntimeMemoryLogTrigger {
            category: "test".to_string(),
            reason: "unit".to_string(),
            session_id: Some("session_test".to_string()),
            detail: None,
        },
        sampling: RuntimeMemoryLogSampling::default(),
        client: ClientRuntimeMemoryClient {
            client_instance_id: "client_test".to_string(),
            session_id: "session_test".to_string(),
            remote_session_id: None,
            provider: "mock".to_string(),
            model: "test-model".to_string(),
            is_remote: false,
            is_processing: false,
            uptime_secs: 1,
        },
        process: crate::process_memory::ProcessMemorySnapshot::default(),
        process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
        totals: ClientRuntimeMemoryTotals::default(),
        session: None,
        ui: None,
        ui_render: None,
        side_panel_render: None,
        markdown: None,
        mermaid: None,
        visual_debug: None,
    };

    let path = append_client_sample(&sample).expect("append client sample");
    assert!(path.starts_with(temp.path()));
    let contents = std::fs::read_to_string(&path).expect("read client log");
    assert!(contents.contains("\"client_test\""));
    assert!(contents.contains("\"session_test\""));

    if let Some(prev) = prev_home {
        crate::env::set_var("JCODE_HOME", prev);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn controller_defers_attribution_until_min_spacing() {
    let config = RuntimeMemoryLogConfig {
        process_interval: Duration::from_secs(60),
        attribution_interval: Duration::from_secs(300),
        attribution_min_spacing: Duration::from_secs(30),
        event_process_min_spacing: Duration::from_secs(5),
        pss_delta_threshold_bytes: 16 * 1024 * 1024,
        attribution_json_delta_threshold_bytes: 4 * 1024 * 1024,
    };
    let mut controller = RuntimeMemoryLogController::new(config);
    let now = Instant::now();
    controller.finalize_attribution_sample(
        now,
        &mut ServerRuntimeMemorySample {
            schema_version: 2,
            kind: "attribution".to_string(),
            timestamp: Utc::now().to_rfc3339(),
            timestamp_ms: Utc::now().timestamp_millis(),
            source: "test".to_string(),
            trigger: RuntimeMemoryLogTrigger {
                category: "startup".to_string(),
                reason: "unit".to_string(),
                session_id: None,
                detail: None,
            },
            sampling: RuntimeMemoryLogSampling::default(),
            server: ServerRuntimeMemoryServer {
                id: "server_test".to_string(),
                name: "test".to_string(),
                icon: "🧪".to_string(),
                version: "v0".to_string(),
                git_hash: "deadbeef".to_string(),
                uptime_secs: 1,
            },
            process: crate::process_memory::ProcessMemorySnapshot::default(),
            process_diagnostics: ServerRuntimeMemoryProcessDiagnostics::default(),
            clients: ServerRuntimeMemoryClients { connected_count: 0 },
            sessions: Some(ServerRuntimeMemorySessions::default()),
            background: ServerRuntimeMemoryBackground { task_count: 0 },
            embeddings: ServerRuntimeMemoryEmbeddings {
                model_available: false,
                stats: crate::embedding::stats(),
            },
        },
    );
    let process = crate::process_memory::ProcessMemorySnapshot::default();
    assert!(
        controller
            .build_sampling_for_attribution(
                now + Duration::from_secs(10),
                &process,
                Some(&RuntimeMemoryLogEvent::new("turn_completed", "turn").force_attribution()),
                None,
            )
            .is_none()
    );
    assert!(
        controller
            .build_sampling_for_attribution(
                now + Duration::from_secs(31),
                &process,
                Some(&RuntimeMemoryLogEvent::new("turn_completed", "turn").force_attribution()),
                None,
            )
            .is_some()
    );
}
