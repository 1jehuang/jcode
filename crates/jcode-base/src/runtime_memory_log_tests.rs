use super::*;

#[test]
fn server_logging_enabled_defaults_on_and_respects_falsey_env() {
    let _guard = crate::storage::lock_test_env();
    let prev = std::env::var_os("KCODE_RUNTIME_MEMORY_LOG");

    crate::env::remove_var("KCODE_RUNTIME_MEMORY_LOG");
    assert!(server_logging_enabled());

    crate::env::set_var("KCODE_RUNTIME_MEMORY_LOG", "0");
    assert!(!server_logging_enabled());

    crate::env::set_var("KCODE_RUNTIME_MEMORY_LOG", "false");
    assert!(!server_logging_enabled());

    crate::env::set_var("KCODE_RUNTIME_MEMORY_LOG", "1");
    assert!(server_logging_enabled());

    if let Some(prev) = prev {
        crate::env::set_var("KCODE_RUNTIME_MEMORY_LOG", prev);
    } else {
        crate::env::remove_var("KCODE_RUNTIME_MEMORY_LOG");
    }
}

#[test]
fn append_server_sample_writes_jsonl_under_memory_logs_dir() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("KCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("KCODE_HOME", temp.path());

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
        crate::env::set_var("KCODE_HOME", prev);
    } else {
        crate::env::remove_var("KCODE_HOME");
    }
}

#[test]
fn append_client_sample_writes_jsonl_under_memory_logs_dir() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("KCODE_HOME");
    let temp = tempfile::TempDir::new().expect("create temp dir");
    crate::env::set_var("KCODE_HOME", temp.path());

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
        crate::env::set_var("KCODE_HOME", prev);
    } else {
        crate::env::remove_var("KCODE_HOME");
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

#[test]
fn allocator_retained_resident_estimate_caps_by_anon_pss_minus_live() {
    // Retained larger than resident anon minus live: cap wins.
    assert_eq!(
        allocator_retained_resident_estimate(
            Some(110 * 1024 * 1024),
            Some(22 * 1024 * 1024),
            Some(127 * 1024 * 1024),
        ),
        Some(105 * 1024 * 1024)
    );
    // Retained smaller than the cap: retained wins.
    assert_eq!(
        allocator_retained_resident_estimate(Some(10), Some(5), Some(100)),
        Some(10)
    );
    // No PSS info: fall back to raw retained.
    assert_eq!(
        allocator_retained_resident_estimate(Some(42), None, None),
        Some(42)
    );
    // No retained stat: absent, not zero.
    assert_eq!(
        allocator_retained_resident_estimate(None, Some(5), Some(100)),
        None
    );
    // Live exceeding anon PSS must not underflow.
    assert_eq!(
        allocator_retained_resident_estimate(Some(50), Some(200), Some(100)),
        Some(0)
    );
}

#[test]
fn thread_stack_estimate_adds_fixed_cost_per_aux_thread() {
    let main_stack = 132 * 1024_u64;
    // 10 threads: main stack + 9 aux stacks at 64KiB each.
    assert_eq!(
        thread_stack_estimate(Some(10), Some(main_stack)),
        Some(main_stack + 9 * 64 * 1024)
    );
    // Single-threaded: just the main stack.
    assert_eq!(
        thread_stack_estimate(Some(1), Some(main_stack)),
        Some(main_stack)
    );
    // Unknown thread count: assume single-threaded.
    assert_eq!(
        thread_stack_estimate(None, Some(main_stack)),
        Some(main_stack)
    );
    // No stack info: absent.
    assert_eq!(thread_stack_estimate(Some(4), None), None);
}

#[test]
fn build_process_diagnostics_populates_coverage_estimates() {
    let process = crate::process_memory::ProcessMemorySnapshot {
        rss_bytes: Some(231 * 1024 * 1024),
        peak_rss_bytes: None,
        virtual_bytes: None,
        thread_count: Some(10),
        main_stack_bytes: Some(132 * 1024),
        os: Some(crate::process_memory::OsProcessMemoryInfo {
            pss_bytes: Some(146 * 1024 * 1024),
            pss_anon_bytes: Some(127 * 1024 * 1024),
            pss_file_bytes: Some(11 * 1024 * 1024),
            pss_shmem_bytes: Some(0),
            anon_huge_pages_bytes: Some(30 * 1024 * 1024),
            rss_anon_bytes: Some(140 * 1024 * 1024),
            ..Default::default()
        }),
        allocator: crate::process_memory::AllocatorInfo {
            name: "system",
            stats_available: true,
            stats: Some(crate::process_memory::AllocatorStats {
                allocated_bytes: Some(22 * 1024 * 1024),
                retained_bytes: Some(110 * 1024 * 1024),
                ..Default::default()
            }),
            tuning: None,
            profiling: None,
        },
    };

    let diagnostics = build_process_diagnostics(&process);
    assert_eq!(
        diagnostics.allocator_retained_resident_estimate_bytes,
        Some(105 * 1024 * 1024)
    );
    assert_eq!(
        diagnostics.thread_stack_estimate_bytes,
        Some(132 * 1024 + 9 * 64 * 1024)
    );
    assert_eq!(
        diagnostics.pss_anon_minus_allocator_allocated_bytes,
        Some((127 - 22) * 1024 * 1024)
    );
}
